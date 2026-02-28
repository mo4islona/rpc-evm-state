use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use alloy_primitives::{hex, Bytes, Address, B256, U256};
use evm_state_common::AccountInfo;
use evm_state_db::StateDb;
use serde::Deserialize;

// ── Error type ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] evm_state_db::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error at line {line}: {source}")]
    Json {
        line: u64,
        source: serde_json::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

// ── Snapshot entry (deserialized from each JSON line) ──────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SnapshotEntry {
    Account {
        address: Address,
        nonce: u64,
        balance: U256,
        #[serde(rename = "codeHash")]
        code_hash: B256,
    },
    Storage {
        address: Address,
        slot: B256,
        value: U256,
    },
    Code {
        #[serde(rename = "codeHash")]
        code_hash: B256,
        bytecode: Bytes,
    },
    Metadata {
        #[serde(rename = "headBlock")]
        head_block: u64,
    },
}

// ── Progress / stats types ─────────────────────────────────────────

/// Progress information passed to the callback during import.
#[derive(Debug, Clone)]
pub struct ImportProgress {
    /// Total entries imported so far (including resumed entries).
    pub entries_imported: u64,
    /// Total bytes read from the file so far.
    pub bytes_read: u64,
}

/// Summary statistics from a completed import.
#[derive(Debug, Clone)]
pub struct ImportStats {
    pub accounts: u64,
    pub storage_slots: u64,
    pub code_entries: u64,
    pub head_block: Option<u64>,
    pub elapsed: Duration,
}

// ── Constants ──────────────────────────────────────────────────────

const BATCH_SIZE: u64 = 10_000;

// ── Import function ────────────────────────────────────────────────

/// Import a JSON Lines snapshot into the state database.
///
/// Supports plain `.jsonl` and zstd-compressed `.jsonl.zst` files.
/// If a previous import was interrupted, it resumes from where it left off
/// using a sidecar `.progress` file.
pub fn import_snapshot(
    db: &StateDb,
    path: &Path,
    mut on_progress: impl FnMut(&ImportProgress),
) -> Result<ImportStats> {
    let start = Instant::now();

    // Check for resume progress.
    let progress_path = path.with_extension(
        path.extension()
            .map(|e| format!("{}.progress", e.to_string_lossy()))
            .unwrap_or_else(|| "progress".to_string()),
    );
    let skip_lines = read_progress(&progress_path);

    // Open file and wrap in appropriate reader.
    let file = File::open(path)?;
    let reader: Box<dyn Read> = if has_zst_extension(path) {
        Box::new(zstd::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let buf_reader = BufReader::new(reader);

    let mut stats = ImportStats {
        accounts: 0,
        storage_slots: 0,
        code_entries: 0,
        head_block: None,
        elapsed: Duration::ZERO,
    };

    let mut line_num: u64 = 0;
    let mut entries_in_batch: u64 = 0;
    let mut bytes_read: u64 = 0;
    let mut batch = db.write_batch()?;

    for line_result in buf_reader.lines() {
        let line = line_result?;
        line_num += 1;
        bytes_read += line.len() as u64 + 1; // +1 for newline

        // Skip already-imported lines on resume.
        if line_num <= skip_lines {
            continue;
        }

        // Skip empty lines.
        if line.trim().is_empty() {
            continue;
        }

        let entry: SnapshotEntry =
            serde_json::from_str(&line).map_err(|e| Error::Json { line: line_num, source: e })?;

        match entry {
            SnapshotEntry::Account { address, nonce, balance, code_hash } => {
                batch.set_account(
                    &address,
                    &AccountInfo { nonce, balance, code_hash },
                )?;
                stats.accounts += 1;
            }
            SnapshotEntry::Storage { address, slot, value } => {
                batch.set_storage(&address, &slot, &value)?;
                stats.storage_slots += 1;
            }
            SnapshotEntry::Code { code_hash, bytecode } => {
                batch.set_code(&code_hash, &bytecode)?;
                stats.code_entries += 1;
            }
            SnapshotEntry::Metadata { head_block } => {
                stats.head_block = Some(head_block);
            }
        }

        entries_in_batch += 1;

        if entries_in_batch >= BATCH_SIZE {
            batch.commit()?;
            write_progress(&progress_path, line_num)?;

            on_progress(&ImportProgress {
                entries_imported: line_num - skip_lines,
                bytes_read,
            });

            entries_in_batch = 0;
            batch = db.write_batch()?;
        }
    }

    // Commit remaining entries.
    if let Some(head_block) = stats.head_block {
        batch.set_head_block(head_block)?;
    }
    batch.commit()?;

    // Clean up progress file.
    let _ = fs::remove_file(&progress_path);

    on_progress(&ImportProgress {
        entries_imported: line_num - skip_lines,
        bytes_read,
    });

    stats.elapsed = start.elapsed();
    Ok(stats)
}

// ── Export types ──────────────────────────────────────────────────

/// Progress information passed to the callback during export.
#[derive(Debug, Clone)]
pub struct ExportProgress {
    /// Total entries written so far.
    pub entries_written: u64,
}

/// Summary statistics from a completed export.
#[derive(Debug, Clone)]
pub struct ExportStats {
    pub accounts: u64,
    pub storage_slots: u64,
    pub code_entries: u64,
    pub head_block: Option<u64>,
    pub elapsed: Duration,
}

// ── Export function ────────────────────────────────────────────────

/// Export the entire state database to a JSON Lines file.
///
/// Supports plain `.jsonl` and zstd-compressed `.jsonl.zst` output
/// (detected by file extension).
pub fn export_snapshot(
    db: &StateDb,
    path: &Path,
    mut on_progress: impl FnMut(&ExportProgress),
) -> Result<ExportStats> {
    let start = Instant::now();

    let file = File::create(path)?;
    let mut writer: Box<dyn Write> = if has_zst_extension(path) {
        Box::new(zstd::Encoder::new(file, 3)?.auto_finish())
    } else {
        Box::new(std::io::BufWriter::new(file))
    };

    let mut stats = ExportStats {
        accounts: 0,
        storage_slots: 0,
        code_entries: 0,
        head_block: None,
        elapsed: Duration::ZERO,
    };
    let mut entries_written: u64 = 0;

    // Accounts
    for (address, info) in db.iter_accounts()? {
        writeln!(
            writer,
            r#"{{"type":"account","address":"0x{:x}","nonce":{},"balance":"0x{:x}","codeHash":"0x{}"}}"#,
            address, info.nonce, info.balance, info.code_hash
        )?;
        stats.accounts += 1;
        entries_written += 1;
        if entries_written % 100_000 == 0 {
            on_progress(&ExportProgress { entries_written });
        }
    }

    // Storage
    for (address, slot, value) in db.iter_storage()? {
        writeln!(
            writer,
            r#"{{"type":"storage","address":"0x{:x}","slot":"0x{}","value":"0x{}"}}"#,
            address, slot, value
        )?;
        stats.storage_slots += 1;
        entries_written += 1;
        if entries_written % 100_000 == 0 {
            on_progress(&ExportProgress { entries_written });
        }
    }

    // Code
    for (code_hash, bytecode) in db.iter_code()? {
        writeln!(
            writer,
            r#"{{"type":"code","codeHash":"0x{}","bytecode":"0x{}"}}"#,
            code_hash,
            hex::encode(&bytecode)
        )?;
        stats.code_entries += 1;
        entries_written += 1;
    }

    // Metadata
    if let Some(head) = db.get_head_block()? {
        writeln!(writer, r#"{{"type":"metadata","headBlock":{}}}"#, head)?;
        stats.head_block = Some(head);
    }

    writer.flush()?;

    on_progress(&ExportProgress { entries_written });

    stats.elapsed = start.elapsed();
    Ok(stats)
}

// ── Helpers ────────────────────────────────────────────────────────

fn has_zst_extension(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".zst")
}

fn read_progress(path: &Path) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn write_progress(path: &Path, line_num: u64) -> Result<()> {
    fs::write(path, line_num.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};
    use std::io::Write;

    fn tmp_db() -> (tempfile::TempDir, StateDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path()).unwrap();
        (dir, db)
    }

    fn sample_snapshot_lines() -> Vec<String> {
        vec![
            r#"{"type":"account","address":"0xd8da6bf26964af9d7eed9e03e53415d37aa96045","nonce":42,"balance":"0x3e8","codeHash":"0x0000000000000000000000000000000000000000000000000000000000000000"}"#.to_string(),
            r#"{"type":"account","address":"0x0000000000000000000000000000000000c0ffee","nonce":0,"balance":"0x0","codeHash":"0xabcdef0000000000000000000000000000000000000000000000000000000001"}"#.to_string(),
            r#"{"type":"storage","address":"0x0000000000000000000000000000000000c0ffee","slot":"0x0000000000000000000000000000000000000000000000000000000000000000","value":"0x000000000000000000000000000000000000000000000000000000000000002a"}"#.to_string(),
            r#"{"type":"storage","address":"0x0000000000000000000000000000000000c0ffee","slot":"0x0000000000000000000000000000000000000000000000000000000000000001","value":"0x00000000000000000000000000000000000000000000000000000000000000ff"}"#.to_string(),
            r#"{"type":"code","codeHash":"0xabcdef0000000000000000000000000000000000000000000000000000000001","bytecode":"0x60426000550060006000fd"}"#.to_string(),
            r#"{"type":"account","address":"0x0000000000000000000000000000000000000001","nonce":100,"balance":"0xde0b6b3a7640000","codeHash":"0x0000000000000000000000000000000000000000000000000000000000000000"}"#.to_string(),
            r#"{"type":"metadata","headBlock":65000000}"#.to_string(),
        ]
    }

    fn write_snapshot_file(dir: &Path, name: &str, lines: &[String]) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut file = File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{}", line).unwrap();
        }
        path
    }

    fn write_zst_snapshot_file(dir: &Path, name: &str, lines: &[String]) -> std::path::PathBuf {
        let path = dir.join(name);
        let file = File::create(&path).unwrap();
        let mut encoder = zstd::Encoder::new(file, 3).unwrap();
        for line in lines {
            writeln!(encoder, "{}", line).unwrap();
        }
        encoder.finish().unwrap();
        path
    }

    fn verify_sample_db(db: &StateDb) {
        // Account 1: vitalik.eth
        let acc1 = db
            .get_account(&address!("d8da6bf26964af9d7eed9e03e53415d37aa96045"))
            .unwrap()
            .unwrap();
        assert_eq!(acc1.nonce, 42);
        assert_eq!(acc1.balance, U256::from(0x3e8u64));
        assert_eq!(acc1.code_hash, B256::ZERO);

        // Account 2: contract
        let contract = address!("0000000000000000000000000000000000c0ffee");
        let acc2 = db.get_account(&contract).unwrap().unwrap();
        assert_eq!(acc2.nonce, 0);
        assert_eq!(
            acc2.code_hash,
            b256!("abcdef0000000000000000000000000000000000000000000000000000000001")
        );

        // Storage
        let slot0 = B256::ZERO;
        let val0 = db.get_storage(&contract, &slot0).unwrap().unwrap();
        assert_eq!(val0, U256::from(0x2au64));

        let slot1 = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        let val1 = db.get_storage(&contract, &slot1).unwrap().unwrap();
        assert_eq!(val1, U256::from(0xffu64));

        // Code
        let code_hash =
            b256!("abcdef0000000000000000000000000000000000000000000000000000000001");
        let code = db.get_code(&code_hash).unwrap().unwrap();
        assert_eq!(
            code,
            alloy_primitives::hex::decode("60426000550060006000fd").unwrap()
        );

        // Account 3: EOA
        let acc3 = db
            .get_account(&address!("0000000000000000000000000000000000000001"))
            .unwrap()
            .unwrap();
        assert_eq!(acc3.nonce, 100);
        assert_eq!(acc3.balance, U256::from(0xde0b6b3a7640000u64));

        // Head block
        assert_eq!(db.get_head_block().unwrap().unwrap(), 65_000_000);
    }

    #[test]
    fn import_small_snapshot() {
        let (_db_dir, db) = tmp_db();
        let snap_dir = tempfile::tempdir().unwrap();
        let lines = sample_snapshot_lines();
        let path = write_snapshot_file(snap_dir.path(), "state.jsonl", &lines);

        let stats = import_snapshot(&db, &path, |_| {}).unwrap();

        assert_eq!(stats.accounts, 3);
        assert_eq!(stats.storage_slots, 2);
        assert_eq!(stats.code_entries, 1);
        assert_eq!(stats.head_block, Some(65_000_000));

        verify_sample_db(&db);
    }

    #[test]
    fn import_zstd_compressed() {
        let (_db_dir, db) = tmp_db();
        let snap_dir = tempfile::tempdir().unwrap();
        let lines = sample_snapshot_lines();
        let path = write_zst_snapshot_file(snap_dir.path(), "state.jsonl.zst", &lines);

        let stats = import_snapshot(&db, &path, |_| {}).unwrap();

        assert_eq!(stats.accounts, 3);
        assert_eq!(stats.storage_slots, 2);
        assert_eq!(stats.code_entries, 1);
        assert_eq!(stats.head_block, Some(65_000_000));

        verify_sample_db(&db);
    }

    #[test]
    fn resume_after_partial_import() {
        let (_db_dir, db) = tmp_db();
        let snap_dir = tempfile::tempdir().unwrap();
        let lines = sample_snapshot_lines();
        let path = write_snapshot_file(snap_dir.path(), "state.jsonl", &lines);

        // Simulate partial import: write progress file indicating 3 lines already done.
        let progress_path = path.with_extension("jsonl.progress");
        fs::write(&progress_path, "3").unwrap();

        // Manually import first 3 lines to simulate previous partial import.
        {
            let batch = db.write_batch().unwrap();
            // Line 1: account vitalik
            batch
                .set_account(
                    &address!("d8da6bf26964af9d7eed9e03e53415d37aa96045"),
                    &AccountInfo {
                        nonce: 42,
                        balance: U256::from(0x3e8u64),
                        code_hash: B256::ZERO,
                    },
                )
                .unwrap();
            // Line 2: account contract
            batch
                .set_account(
                    &address!("0000000000000000000000000000000000c0ffee"),
                    &AccountInfo {
                        nonce: 0,
                        balance: U256::ZERO,
                        code_hash: b256!(
                            "abcdef0000000000000000000000000000000000000000000000000000000001"
                        ),
                    },
                )
                .unwrap();
            // Line 3: storage slot 0
            batch
                .set_storage(
                    &address!("0000000000000000000000000000000000c0ffee"),
                    &B256::ZERO,
                    &U256::from(0x2au64),
                )
                .unwrap();
            batch.commit().unwrap();
        }

        // Now resume import — should skip first 3 lines and import remaining 4.
        let stats = import_snapshot(&db, &path, |_| {}).unwrap();

        // Only the remaining entries are counted.
        assert_eq!(stats.storage_slots, 1); // slot 1 only
        assert_eq!(stats.code_entries, 1);
        assert_eq!(stats.accounts, 1); // account 3 only
        assert_eq!(stats.head_block, Some(65_000_000));

        verify_sample_db(&db);

        // Progress file should be cleaned up.
        assert!(!progress_path.exists());
    }

    #[test]
    fn import_empty_file() {
        let (_db_dir, db) = tmp_db();
        let snap_dir = tempfile::tempdir().unwrap();
        let path = write_snapshot_file(snap_dir.path(), "empty.jsonl", &[]);

        let stats = import_snapshot(&db, &path, |_| {}).unwrap();

        assert_eq!(stats.accounts, 0);
        assert_eq!(stats.storage_slots, 0);
        assert_eq!(stats.code_entries, 0);
        assert!(stats.head_block.is_none());
        assert!(db.get_head_block().unwrap().is_none());
    }

    #[test]
    fn import_invalid_json_line() {
        let (_db_dir, db) = tmp_db();
        let snap_dir = tempfile::tempdir().unwrap();
        let lines = vec![
            r#"{"type":"account","address":"0x0000000000000000000000000000000000000001","nonce":1,"balance":"0x0","codeHash":"0x0000000000000000000000000000000000000000000000000000000000000000"}"#.to_string(),
            "this is not valid json".to_string(),
        ];
        let path = write_snapshot_file(snap_dir.path(), "bad.jsonl", &lines);

        let result = import_snapshot(&db, &path, |_| {});

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            Error::Json { line, .. } => assert_eq!(line, 2),
            other => panic!("expected Json error, got: {other}"),
        }
    }

    #[test]
    fn progress_callback_reports_correctly() {
        let (_db_dir, db) = tmp_db();
        let snap_dir = tempfile::tempdir().unwrap();
        let lines = sample_snapshot_lines();
        let path = write_snapshot_file(snap_dir.path(), "state.jsonl", &lines);

        let mut reports = Vec::new();
        import_snapshot(&db, &path, |p| {
            reports.push(p.clone());
        })
        .unwrap();

        // With 7 entries and batch size 10K, there's 1 final report.
        assert!(!reports.is_empty());
        let last = reports.last().unwrap();
        assert_eq!(last.entries_imported, 7);
        assert!(last.bytes_read > 0);
    }
}
