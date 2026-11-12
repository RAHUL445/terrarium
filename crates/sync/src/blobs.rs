use std::collections::BTreeSet;

use crate::error::Result;
use crate::transport::BatchStore;

const MAX_BLOBS_PER_CYCLE: usize = 50;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlobPlan {
    pub upload: Vec<String>,
    pub download: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlobReport {
    #[serde(default)]
    pub uploaded: usize,
    #[serde(default)]
    pub downloaded: usize,
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub failed: usize,
}

pub trait LocalBlobs {
    fn get_sealed(&self, id: &str) -> Result<Vec<u8>>;
    fn put_sealed(&self, id: &str, bytes: &[u8]) -> Result<()>;
}

pub fn plan(
    local: &BTreeSet<String>,
    remote: &BTreeSet<String>,
    wanted: &dyn Fn(&str) -> bool,
    budget_bytes: u64,
) -> BlobPlan {
    let mut upload: Vec<String> = local.difference(remote).cloned().collect();
    upload.sort();
    upload.truncate(MAX_BLOBS_PER_CYCLE);

    let mut download: Vec<String> = remote
        .difference(local)
        .filter(|id| wanted(id))
        .cloned()
        .collect();
    download.sort();
    let by_budget = budget_count(budget_bytes);
    download.truncate(download.len().min(MAX_BLOBS_PER_CYCLE).min(by_budget));
    BlobPlan { upload, download }
}

fn budget_count(budget_bytes: u64) -> usize {
    if budget_bytes == 0 {
        return 0;
    }
    let each = terrarium_blobs::MAX_BLOB.max(1);
    (budget_bytes / each).min(MAX_BLOBS_PER_CYCLE as u64) as usize
}

pub fn transfer(
    plan: &BlobPlan,
    local: &dyn LocalBlobs,
    remote: &dyn BatchStore,
    report: &mut BlobReport,
) -> Result<()> {
    for id in &plan.upload {
        match local
            .get_sealed(id)
            .and_then(|bytes| remote.blob_put(id, &bytes))
        {
            Ok(()) => report.uploaded += 1,
            Err(_) => report.failed += 1,
        }
    }
    for id in &plan.download {
        match remote
            .blob_get(id)
            .and_then(|bytes| local.put_sealed(id, &bytes))
        {
            Ok(()) => report.downloaded += 1,
            Err(_) => report.failed += 1,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_respects_wanted_and_budget() {
        let local = BTreeSet::from(["a".to_string()]);
        let remote = BTreeSet::from(["a".to_string(), "b".to_string(), "c".to_string()]);
        let plan = plan(&local, &remote, &|id| id == "b", terrarium_blobs::MAX_BLOB);
        assert!(plan.upload.is_empty());
        assert_eq!(plan.download, vec!["b"]);
    }
}
