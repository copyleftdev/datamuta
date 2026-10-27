//! Private, create-once spill directory. Only a fully fsynced capture publishes
//! manifest.json. Merkle proofs authenticate random-access metadata against the
//! approved manifest; restart never hashes the entire completed prefix.
use super::*;
use datamuta_plan::{Record, Scalar};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
};
const SLOT: u64 = 512;
const HASH: u64 = 64;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub partition_rows: u64,
    pub partition_bytes: u64,
    pub spill_bytes: u64,
    pub max_partitions: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            partition_rows: 256,
            partition_bytes: 131072,
            spill_bytes: 268435456,
            max_partitions: 65536,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        require(
            (1..=256).contains(&self.partition_rows)
                && (65537..=1048576).contains(&self.partition_bytes)
                && (8192..=4294967296).contains(&self.spill_bytes)
                && (1..=65536).contains(&self.max_partitions),
            "invalid stream limits",
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub source: Fingerprint,
    pub root: Fingerprint,
    pub rows: u64,
    pub partitions: u64,
    pub payload_bytes: u64,
    pub limits: Limits,
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        self.limits.validate()?;
        require(
            self.version == 1
                && self.partitions > 0
                && self.partitions <= self.limits.max_partitions
                && self.rows >= self.partitions
                && self.rows <= self.partitions * self.limits.partition_rows
                && self.payload_bytes > 0
                && self.payload_bytes <= self.limits.spill_bytes
                && disk_cost(self.payload_bytes, self.partitions) <= self.limits.spill_bytes,
            "invalid spool manifest",
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub index: u64,
    pub first: u64,
    pub rows: u64,
    pub bytes: u64,
    pub payload: Fingerprint,
}
impl Meta {
    pub fn leaf(&self) -> Result<Fingerprint> {
        Ok(canonical_hash(&("datamuta/spool-leaf/v1", self))?)
    }
}
fn pair(a: &Fingerprint, b: &Fingerprint) -> Fingerprint {
    Fingerprint::of(format!("datamuta/spool-node/v1:{}:{}", a.as_str(), b.as_str()).as_bytes())
}
fn disk_cost(payload: u64, n: u64) -> u64 {
    payload + n * (SLOT + HASH) + n.max(1).next_power_of_two() * 2 * HASH + 4096
}
fn fresh(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?)
}
fn checked(path: &Path) -> Result<File> {
    require(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "spool member must be a regular non-symlink file",
    )?;
    Ok(File::open(path)?)
}
fn partition_name(i: u64) -> String {
    format!("partition-{i:08}.jsonl")
}
fn hex(digest: impl AsRef<[u8]>) -> Result<Fingerprint> {
    Ok(Fingerprint::try_from(
        digest
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )?)
}
fn hash_at(f: &mut File, index: u64) -> Result<Fingerprint> {
    f.seek(SeekFrom::Start(index * HASH))?;
    let mut bytes = [0; 64];
    f.read_exact(&mut bytes)?;
    Ok(Fingerprint::try_from(
        String::from_utf8(bytes.to_vec()).map_err(err)?,
    )?)
}
fn put_hash(f: &mut File, index: u64, hash: &Fingerprint) -> Result<()> {
    f.seek(SeekFrom::Start(index * HASH))?;
    f.write_all(hash.as_str().as_bytes())?;
    Ok(())
}
pub struct Builder {
    dir: PathBuf,
    limits: Limits,
    source: Fingerprint,
    keys: Vec<String>,
    last: Option<Vec<i64>>,
    index: File,
    leaves: File,
    part: Option<File>,
    hasher: Sha256,
    rows: u64,
    n: u64,
    part_rows: u64,
    part_bytes: u64,
    payload_bytes: u64,
    failed: bool,
}
impl Builder {
    pub fn create(
        dir: &Path,
        limits: Limits,
        source: Fingerprint,
        keys: Vec<String>,
    ) -> Result<Self> {
        limits.validate()?;
        require(!keys.is_empty() && keys.len() <= 8, "identity arity")?;
        fs::create_dir(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            dir: dir.into(),
            limits,
            source,
            keys,
            last: None,
            index: fresh(&dir.join("index"))?,
            leaves: fresh(&dir.join("leaves"))?,
            part: None,
            hasher: Sha256::new(),
            rows: 0,
            n: 0,
            part_rows: 0,
            part_bytes: 0,
            payload_bytes: 0,
            failed: false,
        })
    }
    pub fn push(&mut self, record: &Record) -> Result<()> {
        require(!self.failed, "spool writer poisoned")?;
        let result = self.push_inner(record);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(&mut self, record: &Record) -> Result<()> {
        let key = identity(record, &self.keys)?;
        require(
            self.last.as_ref().is_none_or(|last| last < &key),
            "source identity must be strictly increasing across partitions",
        )?;
        let mut bytes = serde_json::to_vec(record)?;
        require(bytes.len() <= 65536, "encoded record bound")?;
        bytes.push(b'\n');
        if self.part_rows == self.limits.partition_rows
            || self.part_bytes + bytes.len() as u64 > self.limits.partition_bytes
        {
            self.flush()?;
        }
        require(
            self.n < self.limits.max_partitions
                && disk_cost(
                    self.payload_bytes + self.part_bytes + bytes.len() as u64,
                    self.n + 1,
                ) <= self.limits.spill_bytes,
            "spool disk/partition budget",
        )?;
        if self.part.is_none() {
            self.part = Some(fresh(&self.dir.join(partition_name(self.n)))?);
        }
        self.part
            .as_mut()
            .ok_or_else(|| err("partition file missing"))?
            .write_all(&bytes)?;
        self.hasher.update(&bytes);
        self.part_rows += 1;
        self.part_bytes += bytes.len() as u64;
        self.rows += 1;
        self.last = Some(key);
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.part_rows == 0 {
            return Ok(());
        }
        let file = self
            .part
            .take()
            .ok_or_else(|| err("partition file missing"))?;
        file.sync_all()?;
        let meta = Meta {
            index: self.n,
            first: self.rows - self.part_rows,
            rows: self.part_rows,
            bytes: self.part_bytes,
            payload: hex(std::mem::take(&mut self.hasher).finalize())?,
        };
        let bytes = serde_json::to_vec(&meta)?;
        require(bytes.len() < SLOT as usize, "index slot bound")?;
        let mut slot = [0; SLOT as usize];
        slot[..bytes.len()].copy_from_slice(&bytes);
        self.index.write_all(&slot)?;
        self.leaves.write_all(meta.leaf()?.as_str().as_bytes())?;
        self.n += 1;
        self.payload_bytes += self.part_bytes;
        self.part_rows = 0;
        self.part_bytes = 0;
        Ok(())
    }
    pub fn seal(mut self) -> Result<Manifest> {
        require(
            !self.failed && self.rows > 0,
            "cannot seal failed or empty capture",
        )?;
        self.flush()?;
        self.index.sync_all()?;
        self.leaves.sync_all()?;
        let width = self.n.next_power_of_two();
        let mut tree = fresh(&self.dir.join("tree"))?;
        tree.set_len(width * 2 * HASH)?;
        for i in 0..width {
            let leaf = if i < self.n {
                hash_at(&mut self.leaves, i)?
            } else {
                Fingerprint::of(b"datamuta/spool-padding/v1")
            };
            put_hash(&mut tree, width + i, &leaf)?;
        }
        for i in (1..width).rev() {
            let left = hash_at(&mut tree, 2 * i)?;
            let right = hash_at(&mut tree, 2 * i + 1)?;
            put_hash(&mut tree, i, &pair(&left, &right))?;
        }
        tree.sync_all()?;
        let manifest = Manifest {
            version: 1,
            source: self.source,
            root: hash_at(&mut tree, 1)?,
            rows: self.rows,
            partitions: self.n,
            payload_bytes: self.payload_bytes,
            limits: self.limits,
        };
        manifest.validate()?;
        let bytes = serde_json::to_vec(&manifest)?;
        require(bytes.len() <= 4096, "manifest byte bound")?;
        let mut f = fresh(&self.dir.join("manifest.pending"))?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        File::open(&self.dir)?.sync_all()?;
        fs::rename(
            self.dir.join("manifest.pending"),
            self.dir.join("manifest.json"),
        )?;
        File::open(&self.dir)?.sync_all()?;
        Ok(manifest)
    }
}
pub fn identity(record: &Record, keys: &[String]) -> Result<Vec<i64>> {
    keys.iter()
        .map(|k| match record.get(k) {
            Some(Scalar::I32(v)) => Ok(i64::from(*v)),
            Some(Scalar::I64(v)) => Ok(*v),
            _ => Err(err("integer source identity required")),
        })
        .collect()
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub metadata_reads: u64,
    pub proof_nodes: u64,
    pub payload_reads: u64,
    pub payload_bytes: u64,
    pub peak_payload_bytes: u64,
}
pub struct Reader {
    dir: PathBuf,
    manifest: Manifest,
    index: File,
    tree: File,
    pub metrics: Metrics,
}
impl Reader {
    pub fn open(dir: &Path, expected: &Manifest) -> Result<Self> {
        expected.validate()?;
        let mode = fs::symlink_metadata(dir)?;
        require(
            mode.is_dir() && mode.permissions().mode() & 0o077 == 0,
            "spool directory must be private and not a symlink",
        )?;
        let mut data = Vec::new();
        checked(&dir.join("manifest.json"))?
            .take(4097)
            .read_to_end(&mut data)?;
        require(data.len() <= 4096, "manifest byte bound")?;
        let found: Manifest = serde_json::from_slice(&data)?;
        require(
            found == *expected,
            "snapshot manifest differs from approved plan",
        )?;
        let index = checked(&dir.join("index"))?;
        let tree = checked(&dir.join("tree"))?;
        require(
            index.metadata()?.len() == expected.partitions * SLOT
                && tree.metadata()?.len() == expected.partitions.next_power_of_two() * 2 * HASH,
            "index/tree size mismatch",
        )?;
        Ok(Self {
            dir: dir.into(),
            manifest: found,
            index,
            tree,
            metrics: Metrics::default(),
        })
    }
    pub fn meta(&mut self, i: u64) -> Result<Meta> {
        require(i < self.manifest.partitions, "partition out of range")?;
        self.index.seek(SeekFrom::Start(i * SLOT))?;
        let mut bytes = [0; SLOT as usize];
        self.index.read_exact(&mut bytes)?;
        self.metrics.metadata_reads += 1;
        let end = bytes
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| err("unterminated index slot"))?;
        require(
            bytes[end..].iter().all(|b| *b == 0),
            "index padding changed",
        )?;
        let m: Meta = serde_json::from_slice(&bytes[..end])?;
        require(
            m.index == i
                && m.rows > 0
                && m.rows <= self.manifest.limits.partition_rows
                && m.bytes > 0
                && m.bytes <= self.manifest.limits.partition_bytes
                && m.first <= self.manifest.rows
                && m.rows <= self.manifest.rows - m.first,
            "partition metadata bounds",
        )?;
        let mut hash = m.leaf()?;
        let mut node = self.manifest.partitions.next_power_of_two() + i;
        while node > 1 {
            let sibling = hash_at(&mut self.tree, node ^ 1)?;
            self.metrics.proof_nodes += 1;
            hash = if node.is_multiple_of(2) {
                pair(&hash, &sibling)
            } else {
                pair(&sibling, &hash)
            };
            node /= 2;
        }
        require(
            hash == self.manifest.root,
            "partition Merkle proof mismatch",
        )?;
        Ok(m)
    }
    pub fn records(&mut self, m: &Meta) -> Result<Vec<Record>> {
        require(self.meta(m.index)? == *m, "partition descriptor changed")?;
        let f = checked(&self.dir.join(partition_name(m.index)))?;
        require(f.metadata()?.len() == m.bytes, "partition length mismatch")?;
        let mut bytes = Vec::new();
        f.take(m.bytes + 1).read_to_end(&mut bytes)?;
        require(
            bytes.len() as u64 == m.bytes && Fingerprint::of(&bytes) == m.payload,
            "partition payload hash mismatch",
        )?;
        self.metrics.payload_reads += 1;
        self.metrics.payload_bytes += m.bytes;
        self.metrics.peak_payload_bytes = self.metrics.peak_payload_bytes.max(m.bytes);
        require(bytes.last() == Some(&b'\n'), "incomplete partition frame")?;
        let mut records = Vec::new();
        for line in bytes[..bytes.len() - 1].split(|b| *b == b'\n') {
            require(
                line.len() <= 65536 && records.len() < self.manifest.limits.partition_rows as usize,
                "partition frame bounds",
            )?;
            records.push(serde_json::from_slice(line)?);
        }
        require(
            records.len() as u64 == m.rows,
            "partition record count mismatch",
        )?;
        Ok(records)
    }
}
