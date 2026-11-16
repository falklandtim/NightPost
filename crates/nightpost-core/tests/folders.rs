//! `use_folders`: the iPhone app chooses where the database and cache live.
//!
//! An integration test so the folders, which are set once per process,
//! don't leak into the unit tests.

use nightpost_core::{data_dir, engine, store, use_folders};

#[test]
fn chosen_folders_hold_the_database_and_attachments() {
    let base = std::env::temp_dir().join(format!("nightpost-folders-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (data, cache) = (base.join("data"), base.join("cache"));
    std::env::remove_var("NIGHTPOST_DB");
    std::env::remove_var("NIGHTPOST_CACHE");

    use_folders(data.clone(), cache.clone()).unwrap();
    assert_eq!(data_dir(), data);
    assert!(data.is_dir(), "the data folder is created on first use");
    assert_eq!(store::default_db_path(), data.join("nightpost.db"));
    assert_eq!(engine::attachment_cache_dir(), cache.join("attachments"));

    // A second call can't move the database under a running app.
    assert!(use_folders(base.join("elsewhere"), base.join("elsewhere")).is_err());
    assert_eq!(data_dir(), data);

    let _ = std::fs::remove_dir_all(&base);
}
