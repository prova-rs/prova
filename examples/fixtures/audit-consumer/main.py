#!/usr/bin/env python3
"""Audit consumer service.

Consumes inventory events from a Pulsar topic, records each event as an
audit row in MySQL, and exposes the recorded audits over a tiny REST API.

Configuration (environment variables):
  DATABASE_URL  required  mysql://user:pass@host:port/db
  PULSAR_URL    required  pulsar://host:port
  PORT          required  HTTP listen port (bound on 127.0.0.1)
  PULSAR_TOPIC  optional  topic to consume (default: "inventory-events")

Event payload contract (JSON, one message per item):
  {"item_id": <int>, "display_name": "<string>"}
"""

import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote, urlparse

import pulsar
import pymysql


def die(message):
    sys.stderr.write("audit-consumer: error: %s\n" % message)
    sys.exit(1)


def require_env(name):
    value = os.environ.get(name)
    if not value:
        die("required environment variable %s is not set" % name)
    return value


def parse_database_url(url):
    """Parse mysql://user:pass@host:port/db into PyMySQL connect kwargs."""
    parsed = urlparse(url)
    if parsed.scheme != "mysql":
        die("DATABASE_URL must start with mysql:// (got %r)" % url)
    database = parsed.path.lstrip("/")
    if not database:
        die("DATABASE_URL is missing a database name (mysql://user:pass@host:port/db)")
    return {
        "host": parsed.hostname or "127.0.0.1",
        "port": parsed.port or 3306,
        "user": unquote(parsed.username) if parsed.username else "",
        "password": unquote(parsed.password) if parsed.password else "",
        "database": database,
    }


def consume_forever(consumer, db_conn):
    """Receive events forever, inserting an audit row per event."""
    while True:
        msg = consumer.receive()
        try:
            event = json.loads(msg.data().decode("utf-8"))
            item_id = int(event["item_id"])
            display_name = str(event["display_name"])
            with db_conn.cursor() as cur:
                cur.execute(
                    "INSERT INTO audits (item_id, display_name) VALUES (%s, %s)",
                    (item_id, display_name),
                )
            sys.stderr.write(
                "audit-consumer: recorded event item_id=%d display_name=%r\n"
                % (item_id, display_name)
            )
            consumer.acknowledge(msg)
        except Exception as exc:  # noqa: BLE001 - keep the consumer alive
            sys.stderr.write(
                "audit-consumer: failed to process message %r: %s\n"
                % (msg.data(), exc)
            )
            consumer.negative_acknowledge(msg)


def make_handler(db_config):
    class Handler(BaseHTTPRequestHandler):
        def _send_json(self, status, payload):
            body = json.dumps(payload).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802 - http.server API
            path = urlparse(self.path).path
            if path == "/healthz":
                self._send_json(200, {"status": "ok"})
            elif path == "/audits":
                try:
                    # Fresh, short-lived connection per request: PyMySQL
                    # connections are not thread-safe and the consumer
                    # thread writes concurrently on its own connection.
                    conn = pymysql.connect(autocommit=True, **db_config)
                    try:
                        with conn.cursor() as cur:
                            cur.execute(
                                "SELECT id, item_id, display_name "
                                "FROM audits ORDER BY id"
                            )
                            rows = cur.fetchall()
                    finally:
                        conn.close()
                except Exception as exc:  # noqa: BLE001
                    self._send_json(500, {"error": str(exc)})
                    return
                audits = [
                    {"id": row[0], "item_id": row[1], "display_name": row[2]}
                    for row in rows
                ]
                self._send_json(200, {"audits": audits})
            else:
                self._send_json(404, {"error": "not found"})

        def log_message(self, fmt, *args):
            sys.stderr.write(
                "audit-consumer: http %s - %s\n" % (self.address_string(), fmt % args)
            )

    return Handler


def main():
    database_url = require_env("DATABASE_URL")
    pulsar_url = require_env("PULSAR_URL")
    port_raw = require_env("PORT")
    topic = os.environ.get("PULSAR_TOPIC", "inventory-events")

    try:
        port = int(port_raw)
    except ValueError:
        die("PORT must be an integer (got %r)" % port_raw)

    db_config = parse_database_url(database_url)

    # Connect to MySQL and ensure the audits table exists. Connect once and
    # die on failure: the harness gates on readiness, so no retry loops.
    try:
        db_conn = pymysql.connect(autocommit=True, **db_config)
        with db_conn.cursor() as cur:
            cur.execute(
                "CREATE TABLE IF NOT EXISTS audits ("
                "id BIGINT AUTO_INCREMENT PRIMARY KEY, "
                "item_id BIGINT NOT NULL, "
                "display_name VARCHAR(255) NOT NULL)"
            )
    except Exception as exc:  # noqa: BLE001
        die("failed to connect to MySQL at %s:%d: %s" % (db_config["host"], db_config["port"], exc))

    # Create the Pulsar consumer; die if the broker is unreachable.
    try:
        pulsar_client = pulsar.Client(pulsar_url, operation_timeout_seconds=30)
        consumer = pulsar_client.subscribe(
            topic,
            subscription_name="audit-consumer",
            initial_position=pulsar.InitialPosition.Earliest,
        )
    except Exception as exc:  # noqa: BLE001
        die("failed to subscribe to Pulsar topic %r at %s: %s" % (topic, pulsar_url, exc))

    consumer_thread = threading.Thread(
        target=consume_forever, args=(consumer, db_conn), daemon=True
    )
    consumer_thread.start()

    # Serving HTTP is the readiness signal: HTTP up implies MySQL and Pulsar
    # connections succeeded.
    server = ThreadingHTTPServer(("127.0.0.1", port), make_handler(db_config))
    sys.stderr.write(
        "audit-consumer: listening on http://127.0.0.1:%d (topic=%r)\n" % (port, topic)
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
