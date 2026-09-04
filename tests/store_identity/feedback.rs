use super::*;

#[test]
fn feedback_is_atomic_append_only_canonical_and_persistent() {
    let dir = temp_dir("feedback-durability");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:feedback-durability").unwrap();
    let id = store
        .capture(&capture_decision("Durable feedback"), "repo-a", None)
        .unwrap();

    let first = store.feedback(&id, true).unwrap().unwrap();
    let second = store.feedback(&id, true).unwrap().unwrap();
    assert!((first - 0.55).abs() < 1e-9);
    assert!((second - 0.6).abs() < 1e-9);
    let observer = Connection::open(&path).unwrap();
    let (count, distinct_ids): (i64, i64) = observer
        .query_row(
            "SELECT count(*), count(DISTINCT id) FROM feedback_log WHERE memory_id=?1",
            [&id],
            |record| Ok((record.get(0)?, record.get(1)?)),
        )
        .unwrap();
    assert_eq!((count, distinct_ids), (2, 2));
    let timestamps = observer
        .prepare("SELECT created_at FROM feedback_log WHERE memory_id=?1 ORDER BY rowid")
        .unwrap()
        .query_map([&id], |record| record.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(timestamps.len(), 2);
    timestamps
        .iter()
        .for_each(|value| assert_canonical_utc(value));
    let updated_at: String = observer
        .query_row(
            "SELECT updated_at FROM decisions WHERE id=?1",
            [&id],
            |record| record.get(0),
        )
        .unwrap();
    assert_canonical_utc(&updated_at);

    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert!((reopened.get_record(&id).unwrap().unwrap().effectiveness - 0.6).abs() < 1e-9);

    observer
        .execute_batch(
            "CREATE TRIGGER reject_feedback BEFORE INSERT ON feedback_log
             BEGIN SELECT RAISE(ABORT, 'sensitive sqlite feedback detail'); END;",
        )
        .unwrap();
    let before: (f64, i64, String, i64) = observer
        .query_row(
            "SELECT effectiveness,times_helpful,updated_at,
                    (SELECT count(*) FROM feedback_log WHERE memory_id=?1)
             FROM decisions WHERE id=?1",
            [&id],
            |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                ))
            },
        )
        .unwrap();
    assert!(reopened.feedback(&id, true).is_err());
    let after: (f64, i64, String, i64) = observer
        .query_row(
            "SELECT effectiveness,times_helpful,updated_at,
                    (SELECT count(*) FROM feedback_log WHERE memory_id=?1)
             FROM decisions WHERE id=?1",
            [&id],
            |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(after, before);

    drop(reopened);
    drop(observer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_feedback_writers_append_without_collisions() {
    const WRITERS: usize = 8;
    let dir = temp_dir("feedback-concurrency");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:feedback-concurrency").unwrap();
    let id = store
        .capture(&capture_decision("Concurrent feedback"), "repo-a", None)
        .unwrap();
    drop(store);

    let barrier = Arc::new(Barrier::new(WRITERS));
    let workers: Vec<_> = (0..WRITERS)
        .map(|_| {
            let path = path.clone();
            let id = id.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                barrier.wait();
                store.feedback(&id, true).unwrap().unwrap()
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    let observer = Connection::open(&path).unwrap();
    let (effectiveness, count, distinct_ids): (f64, i64, i64) = observer
        .query_row(
            "SELECT effectiveness,
                    (SELECT count(*) FROM feedback_log WHERE memory_id=?1),
                    (SELECT count(DISTINCT id) FROM feedback_log WHERE memory_id=?1)
             FROM decisions WHERE id=?1",
            [&id],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .unwrap();
    assert!((effectiveness - 0.9).abs() < 1e-9);
    assert_eq!(count, WRITERS as i64);
    assert_eq!(distinct_ids, WRITERS as i64);

    drop(observer);
    std::fs::remove_dir_all(dir).unwrap();
}
