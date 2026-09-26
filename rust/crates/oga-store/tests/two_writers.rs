//! The broker and a CLI command each open the database for writing, as two
//! processes do: two stores over one file.

mod common;

use std::{sync::mpsc, thread, time::Duration};

use common::TestDatabase;

const AT: &str = "2026-01-01T00:00:00.000Z";

#[test]
fn a_write_that_reads_first_waits_for_the_other_writer_instead_of_failing() {
    let database = TestDatabase::new();
    let cli = database.open_writable();
    let broker = database.open_writable();
    let (read, reading) = mpsc::channel();
    let broker_write = thread::spawn(move || {
        reading.recv().expect("the CLI has read");
        broker
            .repositories()
            .settings()
            .put("/project", "broker", "1", AT)
    });

    let written = cli.transaction(|tx| {
        let seen: i64 = tx.query_row("SELECT COUNT(*) FROM cwd_settings", [], |row| row.get(0))?;
        read.send(()).expect("broker waits");
        // Long enough for the broker's write to land between the read and the write below.
        thread::sleep(Duration::from_millis(300));
        tx.execute(
            "INSERT INTO cwd_settings(cwd,key,value,created_at,updated_at) VALUES('/project','cli',?,?,?)",
            rusqlite::params![seen.to_string(), AT, AT],
        )?;
        Ok(seen)
    });

    assert!(written.is_ok(), "{written:?}");
    assert!(
        broker_write.join().expect("broker thread").is_ok(),
        "the broker's write waits its turn"
    );
    let store = database.open_writable();
    let settings = store.repositories().settings();
    assert!(settings.get("/project", "cli").expect("read").is_some());
    assert!(settings.get("/project", "broker").expect("read").is_some());
}
