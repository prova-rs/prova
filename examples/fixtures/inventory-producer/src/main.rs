//! inventory-producer: a small gRPC service backed by Postgres that produces a
//! JSON event to Pulsar for every created item. One half of Prova's
//! "kitchen sink" multi-service example (the Python consumer is the other half).
//!
//! Contract with the consumer: one message per created item on `PULSAR_TOPIC`,
//! payload `{"item_id": <int>, "display_name": "<string>"}`.
//!
//! Startup is fail-fast by design: config comes strictly from env, and if
//! Postgres or Pulsar is unreachable the process exits non-zero immediately —
//! the test harness owns readiness/retries, not the service.

use std::net::SocketAddr;

use pulsar::{Pulsar, TokioExecutor};
use sqlx::postgres::PgPool;
use tonic::{transport::Server, Request, Response, Status};

mod pb {
    tonic::include_proto!("inventory.v1");
}

use pb::inventory_server::{Inventory, InventoryServer};
use pb::{CreateItemRequest, CreateItemResponse, Item, ListItemsRequest, ListItemsResponse};

/// Written by build.rs (protox-compiled); embedded so the gRPC Server
/// Reflection service can expose the schema without any files on disk.
const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/inventory_descriptor.bin"));

struct InventoryService {
    pool: PgPool,
    // pulsar's Producer::send_* takes &mut self; serialize sends with a Mutex.
    producer: tokio::sync::Mutex<pulsar::producer::Producer<TokioExecutor>>,
}

#[tonic::async_trait]
impl Inventory for InventoryService {
    async fn create_item(
        &self,
        request: Request<CreateItemRequest>,
    ) -> Result<Response<CreateItemResponse>, Status> {
        let display_name = request.into_inner().display_name;

        let id: i64 =
            sqlx::query_scalar("INSERT INTO items (display_name) VALUES ($1) RETURNING id")
                .bind(&display_name)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| Status::internal(format!("insert failed: {e}")))?;

        // Produce the event and await the broker receipt BEFORE responding, so
        // callers can rely on the event existing once CreateItem returns.
        let payload =
            serde_json::json!({ "item_id": id, "display_name": display_name }).to_string();
        let receipt = {
            let mut producer = self.producer.lock().await;
            producer
                .send_non_blocking(payload)
                .await
                .map_err(|e| Status::internal(format!("pulsar send failed: {e}")))?
        };
        receipt
            .await
            .map_err(|e| Status::internal(format!("pulsar receipt failed: {e}")))?;

        Ok(Response::new(CreateItemResponse { id, display_name }))
    }

    async fn list_items(
        &self,
        _request: Request<ListItemsRequest>,
    ) -> Result<Response<ListItemsResponse>, Status> {
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, display_name FROM items ORDER BY id")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Status::internal(format!("select failed: {e}")))?;

        let items = rows
            .into_iter()
            .map(|(id, display_name)| Item { id, display_name })
            .collect();

        Ok(Response::new(ListItemsResponse { items }))
    }
}

fn required_env(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => value,
        _ => {
            eprintln!("error: required environment variable {name} is not set");
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = required_env("DATABASE_URL");
    let pulsar_url = required_env("PULSAR_URL");
    let port = required_env("PORT");
    let topic = std::env::var("PULSAR_TOPIC").unwrap_or_else(|_| "inventory-events".to_string());

    let addr: SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .map_err(|e| format!("invalid PORT {port:?}: {e}"))?;

    // Fail fast if any dependency is unreachable — no internal retries.
    let pool = PgPool::connect(&database_url)
        .await
        .map_err(|e| format!("failed to connect to postgres at DATABASE_URL: {e}"))?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (id BIGSERIAL PRIMARY KEY, display_name TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .map_err(|e| format!("failed to create items table: {e}"))?;

    let pulsar = Pulsar::builder(&pulsar_url, TokioExecutor)
        // No internal retries: the client defaults to 12 attempts with up to 30s
        // backoff, which would mask an unreachable broker for minutes. Fail-fast
        // is the contract here — the test harness owns readiness and retries.
        .with_connection_retry_options(pulsar::ConnectionRetryOptions {
            max_retries: 0,
            ..Default::default()
        })
        .build()
        .await
        .map_err(|e| format!("failed to connect to pulsar at PULSAR_URL: {e}"))?;
    let producer = pulsar
        .producer()
        .with_topic(&topic)
        .with_name("inventory-producer")
        .build()
        .await
        .map_err(|e| format!("failed to create pulsar producer on topic {topic:?}: {e}"))?;

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|e| format!("failed to build reflection service: {e}"))?;

    let service = InventoryService {
        pool,
        producer: tokio::sync::Mutex::new(producer),
    };

    println!("inventory-producer listening on {addr} (topic {topic:?})");

    Server::builder()
        .add_service(reflection)
        .add_service(InventoryServer::new(service))
        .serve(addr)
        .await
        .map_err(|e| format!("grpc server failed: {e}"))?;

    Ok(())
}
