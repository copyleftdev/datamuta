use datamuta_kernel::Fingerprint;
use datamuta_plan::{Record, Scalar};
use datamuta_stream::{Result, spool::*};
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    path::Path,
};
fn row(n: i64, bytes: usize) -> Record {
    [
        ("id".into(), Scalar::I64(n)),
        ("text".into(), Scalar::Utf8("x".repeat(bytes))),
    ]
    .into_iter()
    .collect()
}
fn build(path: &Path, n: u64, per: u64) -> Result<Manifest> {
    let mut b = Builder::create(
        path,
        Limits {
            partition_rows: per,
            ..Default::default()
        },
        Fingerprint::of(b"source"),
        vec!["id".into()],
    )?;
    for i in 0..n {
        b.push(&row(i as i64, 100))?;
    }
    b.seal()
}
fn overwrite(path: &Path, offset: u64, bytes: &[u8]) -> Result<()> {
    let mut f = fs::OpenOptions::new().write(true).open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(bytes)?;
    Ok(())
}
#[test]
fn tree_boundaries_random_access_and_bounded_payload() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    for n in 1..=33 {
        let path = tmp.path().join(n.to_string());
        let m = build(&path, n, 1)?;
        let mut r = Reader::open(&path, &m)?;
        for i in (0..n).rev() {
            let meta = r.meta(i)?;
            assert_eq!(meta.first, i);
            assert_eq!(r.records(&meta)?, vec![row(i as i64, 100)]);
        }
        assert!(r.metrics.peak_payload_bytes <= m.limits.partition_bytes);
        assert!(r.metrics.proof_nodes <= 2 * n * 6);
    }
    Ok(())
}
#[test]
fn resume_metadata_reads_no_payload_or_prefix() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("s");
    let m = build(&path, 4097, 1)?;
    let mut r = Reader::open(&path, &m)?;
    r.meta(4096)?;
    assert_eq!(r.metrics.metadata_reads, 1);
    assert_eq!(r.metrics.proof_nodes, 13);
    assert_eq!(r.metrics.payload_reads, 0);
    Ok(())
}
#[test]
fn payload_corruption_and_truncation_refused() -> Result<()> {
    for truncate in [false, true] {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("s");
        let m = build(&path, 3, 2)?;
        let f = path.join("partition-00000000.jsonl");
        if truncate {
            fs::OpenOptions::new().write(true).open(&f)?.set_len(1)?;
        } else {
            overwrite(&f, 0, b"!")?;
        }
        let mut r = Reader::open(&path, &m)?;
        let meta = r.meta(0)?;
        assert!(r.records(&meta).is_err());
    }
    Ok(())
}
#[test]
fn metadata_and_proof_corruption_refused() -> Result<()> {
    for tree in [false, true] {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("s");
        let m = build(&path, 3, 1)?;
        if tree {
            overwrite(&path.join("tree"), 5 * 64, &[b'0'; 64])?;
        } else {
            overwrite(&path.join("index"), 0, b"!")?;
        }
        let mut r = Reader::open(&path, &m)?;
        assert!(r.meta(0).is_err());
    }
    Ok(())
}
#[test]
fn truncated_index_and_changed_manifest_refused() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("s");
    let mut m = build(&path, 3, 1)?;
    m.source = Fingerprint::of(b"another-source");
    assert!(Reader::open(&path, &m).is_err());
    m.source = Fingerprint::of(b"source");
    fs::OpenOptions::new()
        .write(true)
        .open(path.join("index"))?
        .set_len(512)?;
    assert!(Reader::open(&path, &m).is_err());
    Ok(())
}
#[test]
fn interrupted_capture_has_no_seal_and_cannot_be_reused() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("s");
    {
        let mut b = Builder::create(
            &path,
            Limits {
                partition_rows: 1,
                ..Default::default()
            },
            Fingerprint::of(b"s"),
            vec!["id".into()],
        )?;
        b.push(&row(0, 10))?;
        b.push(&row(1, 10))?;
    }
    assert!(!path.join("manifest.json").exists());
    assert!(
        Builder::create(
            &path,
            Limits::default(),
            Fingerprint::of(b"s"),
            vec!["id".into()]
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn ordering_failure_poisons_capture_across_partition_boundary() -> Result<()> {
    for second in [0, -1] {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("s");
        let mut b = Builder::create(
            &path,
            Limits {
                partition_rows: 1,
                ..Default::default()
            },
            Fingerprint::of(b"s"),
            vec!["id".into()],
        )?;
        b.push(&row(0, 10))?;
        assert!(b.push(&row(second, 10)).is_err());
        assert!(b.push(&row(1, 10)).is_err());
        assert!(b.seal().is_err());
        assert!(!path.join("manifest.json").exists());
    }
    Ok(())
}
#[test]
fn quota_and_oversized_record_fail_without_seal() -> Result<()> {
    for oversized in [false, true] {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("s");
        let limits = Limits {
            partition_rows: 1,
            max_partitions: 1,
            ..Default::default()
        };
        let mut b = Builder::create(&path, limits, Fingerprint::of(b"s"), vec!["id".into()])?;
        if oversized {
            assert!(b.push(&row(0, 65536)).is_err());
        } else {
            b.push(&row(0, 1))?;
            assert!(b.push(&row(1, 1)).is_err());
        }
        assert!(b.seal().is_err());
        assert!(!path.join("manifest.json").exists());
    }
    Ok(())
}
#[test]
fn byte_credit_flushes_before_row_limit() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("s");
    let mut b = Builder::create(
        &path,
        Limits {
            partition_bytes: 65537,
            ..Default::default()
        },
        Fingerprint::of(b"s"),
        vec!["id".into()],
    )?;
    for i in 0..10 {
        b.push(&row(i, 20000))?;
    }
    let m = b.seal()?;
    assert_eq!(m.partitions, 4);
    let mut r = Reader::open(&path, &m)?;
    for i in 0..4 {
        let meta = r.meta(i)?;
        assert!(meta.bytes <= 65537);
        assert!(meta.rows <= 3);
    }
    Ok(())
}
#[test]
fn disk_budget_and_extreme_untrusted_manifest_are_checked() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("s");
    let mut b = Builder::create(
        &path,
        Limits {
            spill_bytes: 8192,
            ..Default::default()
        },
        Fingerprint::of(b"s"),
        vec!["id".into()],
    )?;
    assert!(b.push(&row(0, 7000)).is_err());
    assert!(b.seal().is_err());
    let path = tmp.path().join("good");
    let mut m = build(&path, 1, 1)?;
    m.payload_bytes = u64::MAX;
    assert!(m.validate().is_err());
    m.partitions = u64::MAX;
    assert!(m.validate().is_err());
    Ok(())
}
#[test]
fn symlink_payload_and_public_directory_refused() -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("s");
    let m = build(&path, 1, 1)?;
    let p = path.join("partition-00000000.jsonl");
    let moved = tmp.path().join("other");
    fs::rename(&p, &moved)?;
    symlink(&moved, &p)?;
    let mut r = Reader::open(&path, &m)?;
    let meta = r.meta(0)?;
    assert!(r.records(&meta).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    assert!(Reader::open(&path, &m).is_err());
    Ok(())
}
