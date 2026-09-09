use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;

use oga_domain::{MemoryEntry, Profile, Provider};
use oga_store::{Store, StoreError};
use tempfile::tempdir;

fn profile(id: &str) -> Profile {
    Profile {
        id: id.into(),
        label: id.into(),
        provider: Provider::Claude,
        default_model: "model".into(),
        enabled: true,
        env: BTreeMap::new(),
        capabilities: vec![],
        command: None,
    }
}

#[test]
fn repositories_round_trip_profiles_and_memories() {
    let directory = tempdir().unwrap();
    let store = Store::open_writable(directory.path().join("oga.db")).unwrap();
    let repos = store.repositories();
    repos
        .profiles()
        .insert(&profile("p1"), "2026-01-01T00:00:00Z")
        .unwrap();
    assert_eq!(repos.profiles().get("p1").unwrap().unwrap(), profile("p1"));
    let mut updated = profile("p1");
    updated.label = "Updated".into();
    updated.enabled = false;
    assert!(
        repos
            .profiles()
            .update_if_unchanged("p1", &profile("p1"), &updated, "2026-01-02T00:00:00Z",)
            .unwrap()
    );
    assert_eq!(repos.profiles().get("p1").unwrap().unwrap(), updated);

    let mut stale = profile("p1");
    stale.label = "Stale update".into();
    assert!(
        !repos
            .profiles()
            .update_if_unchanged("p1", &profile("p1"), &stale, "2026-01-02T00:00:01Z",)
            .unwrap()
    );
    assert_eq!(repos.profiles().get("p1").unwrap().unwrap(), updated);

    assert!(
        repos
            .profiles()
            .remove("p1", "2026-01-03T00:00:00Z")
            .unwrap()
    );
    assert!(repos.profiles().get("p1").unwrap().is_none());

    let entry = MemoryEntry {
        cwd: "/tmp/project".into(),
        key: "rule".into(),
        value: "value".into(),
        version: 1,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    repos.memories().upsert(&entry, None).unwrap();
    let updated = MemoryEntry {
        value: "new".into(),
        updated_at: "2026-01-02T00:00:00Z".into(),
        ..entry.clone()
    };
    assert_eq!(
        repos.memories().upsert(&updated, Some(1)).unwrap().version,
        2
    );
    assert!(matches!(
        repos.memories().upsert(&updated, Some(1)),
        Err(StoreError::Refusal(_))
    ));
}

#[test]
fn writer_serializes_concurrent_transactions() {
    let directory = tempdir().unwrap();
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).unwrap());
    let mut threads = Vec::new();
    for index in 0..8 {
        let store = Arc::clone(&store);
        threads.push(thread::spawn(move || {
            let id = format!("p{index}");
            store
                .repositories()
                .profiles()
                .insert(&profile(&id), "2026-01-01T00:00:00Z")
                .unwrap();
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(store.repositories().profiles().list().unwrap().len(), 8);
}

#[test]
fn failed_transaction_does_not_publish_partial_writes() {
    let directory = tempdir().unwrap();
    let store = Store::open_writable(directory.path().join("oga.db")).unwrap();
    let result: Result<(), StoreError> = store.transaction(|tx| {
        tx.execute("INSERT INTO settings(key,value) VALUES('one','1')", [])
            .unwrap();
        Err(StoreError::Refusal("stop".into()))
    });
    assert!(result.is_err());
    assert!(
        store
            .with_connection(|connection| {
                let count: i64 =
                    connection.query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))?;
                Ok(count)
            })
            .unwrap()
            == 0
    );
}
