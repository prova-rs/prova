//! `t:advise{ family, was, now }`: an advisory measure a test reports beside its outcome, never changing
//! it. A census the User ruled advisory (Substrate's, mail:516) used to say what grew only in a skip's
//! words, which nobody read; a reader (Herald) now reads the `advice` event by kind (D, 2026-11-10).

use prova_core::model::event_to_json;
use prova_core::{run_path, Event, Outcome, Reporter};

/// Every event, as the JSON reporter writes it, so the proof reads the wire a reader reads.
#[derive(Default)]
struct Wire(Vec<serde_json::Value>);

impl Reporter for Wire {
    fn event(&mut self, event: &Event) {
        self.0.push(event_to_json(event));
    }
}

fn run(source: &str) -> (prova_core::Summary, Vec<serde_json::Value>) {
    // One directory per source, under the target's own temp space: no extra dependency for a scratch file.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "prova-advise-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    let file = dir.join("advise.prova.lua");
    std::fs::write(&file, source).expect("proof");
    let mut wire = Wire::default();
    let summary = run_path(&file, &mut wire).expect("ran");
    std::fs::remove_dir_all(&dir).ok();
    (summary, wire.0)
}

fn of_type<'a>(events: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["type"] == kind).collect()
}

#[test]
fn an_advisory_census_reports_each_family_that_moved_and_still_only_skips() {
    let (summary, events) = run(r#"
prova.test("the census", function(t)
    t:advise({ family = "leg_vocabulary", was = 2798, now = 2807 })
    t:advise({ family = "attach_relation", was = 106, now = 108 })
    t:skip("advisory: two families grew")
end)
"#);
    assert_eq!(
        (summary.skipped, summary.failed, summary.passed),
        (1, 0, 0),
        "the verdict is the skip's alone"
    );
    let advice = of_type(&events, "advice");
    assert_eq!(advice.len(), 2, "{events:#?}");
    assert_eq!(advice[0]["path"], "the census");
    assert_eq!(
        (&advice[0]["family"], &advice[0]["was"], &advice[0]["now"]),
        (&"leg_vocabulary".into(), &2798.into(), &2807.into())
    );
    assert_eq!(advice[1]["family"], "attach_relation");
    // Sent before the test's finish, so a reader holding a node's advice knows it when the node ends.
    let at = |e: &serde_json::Value| {
        events
            .iter()
            .position(|x| std::ptr::eq(x, e))
            .expect("in the stream")
    };
    let finished = events
        .iter()
        .position(|e| e["type"] == "node_finished")
        .expect("finished");
    assert!(
        at(advice[1]) < finished,
        "advice precedes its node_finished"
    );
    assert_eq!(events[finished]["outcome"], Outcome::Skipped.as_str());
}

#[test]
fn advice_never_changes_a_passing_tests_verdict() {
    let (summary, events) = run(r#"
prova.test("shrank", function(t)
    t:advise({ family = "unbounded_reads", was = 27, now = 25 })
    t:expect(1):equals(1)
end)
"#);
    assert_eq!((summary.passed, summary.failed, summary.skipped), (1, 0, 0));
    assert_eq!(of_type(&events, "advice").len(), 1);
}

#[test]
fn advice_without_a_family_or_a_number_is_the_proofs_own_error() {
    let (summary, events) = run(r#"
prova.test("no family", function(t) t:advise({ was = 1, now = 2 }) end)
prova.test("no now", function(t) t:advise({ family = "x", was = 1 }) end)
"#);
    assert_eq!(summary.failed, 2, "{events:#?}");
    let messages: Vec<String> = of_type(&events, "node_finished")
        .iter()
        .map(|e| e["message"].to_string())
        .collect();
    assert!(messages[0].contains("needs a family"), "{messages:?}");
    assert!(messages[1].contains("needs now"), "{messages:?}");
    assert!(
        of_type(&events, "advice").is_empty(),
        "nothing half-recorded"
    );
}
