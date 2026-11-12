//! Device-local network audit. Storage lives with the queue so admission,
//! workers and Settings share a single bounded writer without a host/runtime
//! dependency cycle. It is never registered as a synced store.
pub use terrarium_runtime::net::Audit;

#[cfg(test)]
mod tests {
    #[test]
    fn audit_has_host_but_no_path_or_query() {
        use terrarium_runtime::net::{fetch, HostRules, Net, Request};
        let dir = tempfile::tempdir().unwrap();
        let mut net = Net::open(dir.path()).unwrap();
        let request=Request::parse(r#"{"method":"GET","url":"https://example.com/private-path?private-query=value","headers":{"X-Custom":"private-header"},"context":{"secret":"private-context"}}"#).unwrap();
        net.submit(
            "personal",
            "fixture",
            "done",
            request,
            None,
            &HostRules::new(&["example.com".into()]).unwrap(),
        )
        .unwrap();
        let mut job = net.take().unwrap();
        job.queued_ms = 0;
        assert_eq!(fetch::run(&job, None).unwrap_err().code, "timeout");
        let text = std::fs::read_to_string(dir.path().join("network-audit.jsonl")).unwrap();
        assert!(text.contains("example.com"));
        for secret in [
            "private-path",
            "private-query",
            "private-header",
            "private-context",
            "https:",
        ] {
            assert!(!text.contains(secret));
        }
        assert_eq!(net.activity("fixture", false).unwrap().len(), 1);
        assert!(net.activity("fixture", true).unwrap().is_empty());
    }
}
