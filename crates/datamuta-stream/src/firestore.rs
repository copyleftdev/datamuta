//! Loopback Firestore emulator profile. Native REST values, atomic Commit with a
//! cursor update-time precondition, and bounded reads at one pinned server readTime.
//! One explicitly selected flat collection; canonical integer document IDs only.
use crate::publisher::{Boundary, Cursor, cert, check_cursor};
use crate::*;
use datamuta_plan::{DataType, Record, Scalar};
use serde_json::{Value, json};
use std::time::Duration;

pub const DATABASE: &str = "projects/demo-datamuta/databases/(default)";
const RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const COMMIT_BYTES: usize = 2 * 1024 * 1024;
const DOCUMENT_BYTES: usize = 65536;
const PAGE: usize = 16;

/// Lab-only transport also used by the gym's independent native-value oracle.
/// No credentials, environment endpoint override, proxy, redirect or auto retry.
pub struct Lab {
    client: reqwest::Client,
    origin: String,
    pub parent: String,
}
impl Lab {
    pub fn new(e: &Endpoint) -> Result<Self> {
        e.validate()?;
        require(
            e.lab_only && e.verified.is_none(),
            "Firestore requires loopback lab profile",
        )?;
        Ok(Self {
            client: reqwest::Client::builder()
                .tls_backend_preconfigured(
                    rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
                        rustls::crypto::ring::default_provider(),
                    ))
                    .with_safe_default_protocol_versions()
                    .map_err(err)?
                    .with_root_certificates(rustls::RootCertStore::empty())
                    .with_no_client_auth(),
                )
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(err)?,
            origin: format!("http://127.0.0.1:{}/v1/", e.port),
            parent: format!("{DATABASE}/documents/{}/root", e.namespace),
        })
    }
    pub fn name(&self, collection: &str, id: &str) -> String {
        format!("{}/{collection}/{id}", self.parent)
    }
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Option<Value>> {
        let mut response = request.bearer_auth("owner").send().await.map_err(err)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(err)? {
            require(
                chunk.len() <= RESPONSE_BYTES.saturating_sub(bytes.len()),
                "Firestore response byte bound",
            )?;
            bytes.extend_from_slice(&chunk);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let category = match error["error"]["status"].as_str() {
                Some("FAILED_PRECONDITION") => "FAILED_PRECONDITION",
                Some("ABORTED") => "ABORTED",
                Some("INVALID_ARGUMENT") => "INVALID_ARGUMENT",
                Some("PERMISSION_DENIED") => "PERMISSION_DENIED",
                Some("RESOURCE_EXHAUSTED") => "RESOURCE_EXHAUSTED",
                _ => "OTHER",
            };
            return Err(err(format!(
                "Firestore HTTP {} ({category})",
                status.as_u16()
            )));
        }
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
    pub async fn get(&self, name: &str, query: &[(&str, &str)]) -> Result<Option<Value>> {
        require(
            name.starts_with(&format!("{DATABASE}/documents/")),
            "Firestore resource prefix",
        )?;
        self.send(
            self.client
                .get(format!("{}{name}", self.origin))
                .query(query),
        )
        .await
    }
    pub async fn post(&self, name: &str, body: &Value) -> Result<Value> {
        require(
            name.starts_with(&format!("{DATABASE}/documents")),
            "Firestore resource prefix",
        )?;
        let bytes = serde_json::to_vec(body)?;
        require(bytes.len() <= COMMIT_BYTES, "Firestore request byte bound")?;
        self.send(
            self.client
                .post(format!("{}{name}", self.origin))
                .header("Content-Type", "application/json")
                .body(bytes),
        )
        .await?
        .ok_or_else(|| err("Firestore resource missing"))
    }
    pub async fn commit(&self, writes: Vec<Value>) -> Result<Value> {
        require(
            !writes.is_empty() && writes.len() <= 258,
            "Firestore commit write bound",
        )?;
        self.post(
            &format!("{DATABASE}/documents:commit"),
            &json!({"writes":writes}),
        )
        .await
    }
    pub async fn snapshot(&self, collection: &str) -> Result<String> {
        let v=self.post(&format!("{}:runQuery",self.parent),&json!({"structuredQuery":{
            "from":[{"collectionId":collection}],"select":{"fields":[{"fieldPath":"__name__"}]},"limit":1
        }})).await?;
        let rows = v
            .as_array()
            .ok_or_else(|| err("Firestore snapshot response"))?;
        require(rows.len() == 1, "Firestore snapshot response count")?;
        Ok(string(&rows[0], "readTime")?.into())
    }
    pub async fn snapshot_page(
        &self,
        collection: &str,
        after: &str,
        time: &str,
    ) -> Result<Vec<Value>> {
        let mut query = json!({"from":[{"collectionId":collection}],
            "orderBy":[{"field":{"fieldPath":"__name__"},"direction":"ASCENDING"}],"limit":16});
        if !after.is_empty() {
            query["startAt"] = json!({"values":[{"referenceValue":after}],"before":false});
        }
        let response = self
            .post(
                &format!("{}:runQuery", self.parent),
                &json!({"structuredQuery":query,"readTime":time}),
            )
            .await?;
        let rows = response
            .as_array()
            .ok_or_else(|| err("Firestore query response"))?;
        require(rows.len() <= PAGE + 1, "Firestore query response count")?;
        let mut docs = Vec::new();
        for row in rows {
            require(
                string(row, "readTime")? == time,
                "Firestore snapshot time changed",
            )?;
            if let Some(doc) = row.get("document") {
                docs.push(doc.clone());
            }
        }
        require(docs.len() <= PAGE, "Firestore page row bound")?;
        Ok(docs)
    }
    pub async fn page(&self, collection: &str, token: &str, time: &str) -> Result<Value> {
        require(token.len() <= 8192, "Firestore pagination token bound")?;
        let mut query = vec![
            ("pageSize", "16"),
            ("showMissing", "true"),
            ("pageToken", token),
        ];
        if !time.is_empty() {
            query.push(("readTime", time));
        }
        self.get(&format!("{}/{collection}", self.parent), &query)
            .await?
            .ok_or_else(|| err("Firestore collection listing unavailable"))
    }
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| err(format!("Firestore missing string: {key}")))
}
/// Lexicographic ID order equals the signed, composite integer key order.
pub fn identity(schema: &Schema, record: &Record) -> Result<String> {
    schema
        .primary_key
        .iter()
        .map(|k| {
            let n = match record.get(k) {
                Some(Scalar::I32(n)) => i64::from(*n),
                Some(Scalar::I64(n)) => *n,
                _ => return Err(err("Firestore integer identity required")),
            };
            Ok(format!("{:016x}", (n as u64) ^ (1_u64 << 63)))
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("-"))
}
pub fn fields(record: &Record) -> Value {
    Value::Object(
        record
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        Scalar::Bool(n) => json!({"booleanValue":n}),
                        Scalar::I32(n) => json!({"integerValue":n.to_string()}),
                        Scalar::I64(n) => json!({"integerValue":n.to_string()}),
                        Scalar::Utf8(s) => json!({"stringValue":s}),
                        Scalar::Null => json!({"nullValue":null}),
                    },
                )
            })
            .collect(),
    )
}
fn decode(schema: &Schema, doc: &Value) -> Result<Record> {
    require(
        serde_json::to_vec(doc)?.len() <= DOCUMENT_BYTES,
        "Firestore document byte bound",
    )?;
    let values = doc
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| err("Firestore missing document fields"))?;
    require(
        values.len() == schema.fields.len(),
        "Firestore missing or extra field",
    )?;
    let mut record = Record::new();
    for f in &schema.fields {
        let value = values
            .get(&f.name)
            .and_then(Value::as_object)
            .ok_or_else(|| err("Firestore missing or invalid field"))?;
        require(value.len() == 1, "Firestore invalid value union")?;
        let value = Value::Object(value.clone());
        let scalar = if value.get("nullValue") == Some(&Value::Null) {
            Scalar::Null
        } else {
            match f.data_type {
                DataType::Bool => Scalar::Bool(
                    value
                        .get("booleanValue")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| err("Firestore boolean type required"))?,
                ),
                DataType::I32 => Scalar::I32(string(&value, "integerValue")?.parse().map_err(err)?),
                DataType::I64 => Scalar::I64(string(&value, "integerValue")?.parse().map_err(err)?),
                DataType::Utf8 => Scalar::Utf8(string(&value, "stringValue")?.into()),
            }
        };
        record.insert(f.name.clone(), scalar);
    }
    schema.check_record(&record)?;
    Ok(record)
}
fn cursor_value(doc: &Value) -> Result<Cursor> {
    let f = doc
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| err("Firestore cursor fields"))?;
    require(f.len() == 3, "Firestore cursor schema mismatch")?;
    let next: u64 = string(
        f.get("next")
            .ok_or_else(|| err("Firestore cursor next missing"))?,
        "integerValue",
    )?
    .parse()
    .map_err(err)?;
    Ok(Cursor {
        next,
        plan: string(
            f.get("plan")
                .ok_or_else(|| err("Firestore cursor plan missing"))?,
            "stringValue",
        )?
        .into(),
        certificate: string(
            f.get("certificate")
                .ok_or_else(|| err("Firestore cursor certificate missing"))?,
            "stringValue",
        )?
        .into(),
    })
}
pub async fn admit_config(config: &Config) -> Result<()> {
    let lab = Lab::new(&config.target_endpoint)?;
    let d = lab
        .get(&lab.name("stream_cursor", "1"), &[])
        .await?
        .ok_or_else(|| err("Firestore cursor missing"))?;
    let _ = cursor_value(&d)?;
    Ok(())
}
pub async fn capture(
    config: Config,
    path: &Path,
    mut observe: impl FnMut(u64) -> Result<()>,
) -> Result<Plan> {
    require(
        config.source_engine == SourceEngine::Firestore,
        "Firestore source engine required",
    )?;
    let mapping = config.compile()?;
    publisher::admit_target(&config).await?;
    let lab = Lab::new(&config.source_endpoint)?;
    let schema = mapping.source();
    // ListDocuments is used only for a structural preflight, including orphan
    // parents (queries omit those). This is a live check; the data snapshot below
    // is the explicitly selected collection, not a recursive subtree snapshot.
    let mut structural_token = String::new();
    let mut inspected = 0_u64;
    loop {
        let page = lab.page(&schema.relation, &structural_token, "").await?;
        let docs = page
            .get("documents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        require(docs.len() <= PAGE, "Firestore page row bound")?;
        let page_len = docs.len();
        for doc in docs {
            let record = decode(schema, &doc)?;
            let name = lab.name(&schema.relation, &identity(schema, &record)?);
            require(
                string(&doc, "name")? == name,
                "Firestore noncanonical document identity",
            )?;
            let children = lab
                .post(&format!("{name}:listCollectionIds"), &json!({"pageSize":1}))
                .await?;
            require(
                children
                    .get("collectionIds")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty)
                    && children
                        .get("nextPageToken")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty),
                "Firestore subcollections unsupported",
            )?;
            inspected += 1;
            require(
                inspected <= config.limits.max_partitions * config.limits.partition_rows,
                "Firestore preflight row bound",
            )?;
        }
        let next = page
            .get("nextPageToken")
            .and_then(Value::as_str)
            .unwrap_or("");
        if next.is_empty() {
            break;
        }
        require(
            page_len > 0 && next != structural_token,
            "Firestore pagination failed to advance",
        )?;
        structural_token = next.into();
    }
    let time = lab.snapshot(&schema.relation).await?;
    let source = canonical_hash(&(
        "firestore/read-time/v1",
        DATABASE,
        config.source_endpoint.receipt_domain()?,
        schema,
        &time,
    ))?;
    let mut writer = spool::Builder::create(
        path,
        config.limits.clone(),
        source,
        schema.primary_key.clone(),
    )?;
    let mut token = String::new();
    let mut count = 0;
    // REST RunQuery carries the Timestamp in its JSON body. The pinned
    // emulator incorrectly decodes timestamp/bytes in GET query parameters.
    loop {
        let docs = lab.snapshot_page(&schema.relation, &token, &time).await?;
        let page_len = docs.len();
        for doc in docs {
            let record = decode(schema, &doc)?;
            let name = lab.name(&schema.relation, &identity(schema, &record)?);
            require(
                string(&doc, "name")? == name && (token.is_empty() || name > token),
                "Firestore noncanonical or unordered identity",
            )?;
            let _ = mapping.prepare(std::slice::from_ref(&record))?;
            writer.push(&record)?;
            token = name;
            count += 1;
            observe(count)?;
        }
        if page_len < PAGE {
            break;
        }
    }
    Plan::new(config, writer.seal()?)
}
pub struct Destination {
    lab: Lab,
    mapping: CompiledMapping,
    plan: Fingerprint,
    partitions: u64,
}
impl Destination {
    pub async fn open(plan: &Plan) -> Result<Self> {
        let mapping = plan.validate()?;
        admit_config(&plan.config).await?;
        Ok(Self {
            lab: Lab::new(&plan.config.target_endpoint)?,
            mapping,
            plan: plan.fingerprint()?,
            partitions: plan.snapshot.partitions,
        })
    }
    async fn state(&self) -> Result<(Cursor, String)> {
        let doc = self
            .lab
            .get(&self.lab.name("stream_cursor", "1"), &[])
            .await?
            .ok_or_else(|| err("Firestore cursor missing"))?;
        Ok((
            check_cursor(cursor_value(&doc)?, &self.plan, self.partitions)?,
            string(&doc, "updateTime")?.into(),
        ))
    }
    pub(crate) async fn cursor(&self) -> Result<Cursor> {
        Ok(self.state().await?.0)
    }
    pub(crate) async fn receipt(&self, i: u64) -> Result<Option<String>> {
        self.lab
            .get(&self.lab.name("stream_receipts", &i.to_string()), &[])
            .await?
            .map(|d| string(&d["fields"]["hash"], "stringValue").map(str::to_owned))
            .transpose()
    }
    pub(crate) async fn apply(
        &mut self,
        meta: &spool::Meta,
        records: &[Record],
        observe: &mut impl FnMut(Boundary, u64) -> Result<()>,
    ) -> Result<bool> {
        let units = self.mapping.prepare(records)?;
        require(units.len() as u64 == meta.rows, "partition rows differ")?;
        let (state, time) = self.state().await?;
        let expected = cert(&self.plan, meta)?;
        if state.next > meta.index {
            require(
                self.receipt(meta.index).await?.as_deref() == Some(expected.as_str()),
                "replayed partition receipt conflict",
            )?;
            return Ok(false);
        }
        require(state.next == meta.index, "noncontiguous partition attempt")?;
        require(
            self.receipt(meta.index).await?.is_none(),
            "receipt exists ahead of atomic cursor",
        )?;
        let mut writes = Vec::with_capacity(units.len() + 2);
        for unit in units {
            let name = self.lab.name(
                &self.mapping.target().relation,
                &identity(self.mapping.target(), unit.fields())?,
            );
            let document = json!({"name":name,"fields":fields(unit.fields())});
            require(
                serde_json::to_vec(&document)?.len() <= DOCUMENT_BYTES,
                "Firestore document byte bound",
            )?;
            writes.push(json!({"update":document,"currentDocument":{"exists":false}}));
        }
        writes.push(json!({"update":{"name":self.lab.name("stream_receipts", &meta.index.to_string()),"fields":{"hash":{"stringValue":expected.as_str()}}},"currentDocument":{"exists":false}}));
        writes.push(json!({"update":{"name":self.lab.name("stream_cursor","1"),"fields":{
            "plan":{"stringValue":self.plan.as_str()},"next":{"integerValue":(meta.index+1).to_string()},"certificate":{"stringValue":expected.as_str()}
        }},"currentDocument":{"updateTime":time}}));
        // CAS fences competing publishers in the same atomic Commit as data and
        // receipt. Unknown outcomes propagate; a fresh run consults the receipt.
        require(
            serde_json::to_vec(&json!({"writes":&writes}))?.len() <= COMMIT_BYTES,
            "Firestore request byte bound",
        )?;
        observe(Boundary::BeforeCommit, meta.index)?;
        self.lab.commit(writes).await?;
        observe(Boundary::AfterCommit, meta.index)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn schema() -> Schema {
        Schema {
            version: 1,
            namespace: "dm_test".into(),
            relation: "records".into(),
            primary_key: vec!["id".into()],
            fields: vec![
                datamuta_plan::Field {
                    name: "id".into(),
                    data_type: DataType::I64,
                    nullable: false,
                },
                datamuta_plan::Field {
                    name: "note".into(),
                    data_type: DataType::Utf8,
                    nullable: true,
                },
            ],
        }
    }
    #[test]
    fn signed_extremes_sort_and_native_values_are_lossless() -> Result<()> {
        let s = schema();
        let mut ids = Vec::new();
        for n in [i64::MIN, -1, 0, 1, i64::MAX] {
            let r = Record::from([
                ("id".into(), Scalar::I64(n)),
                ("note".into(), Scalar::Utf8("雪\r\n\u{301}".into())),
            ]);
            ids.push(identity(&s, &r)?);
            assert_eq!(decode(&s, &json!({"fields":fields(&r)}))?, r);
        }
        assert!(ids.windows(2).all(|p| p[0] < p[1]));
        Ok(())
    }
    #[test]
    fn malformed_lossy_and_oversized_values_fail_closed() {
        let s = schema();
        for v in [
            json!({"doubleValue":9_007_199_254_740_992_f64}),
            json!({"integerValue":"9223372036854775808"}),
            json!({"integerValue":"0","doubleValue":0.0}),
            json!({"nullValue":null}),
        ] {
            assert!(decode(&s, &json!({"fields":{"id":v,"note":{"nullValue":null}}})).is_err());
        }
        assert!(decode(&s, &json!({"fields":{"id":{"integerValue":"0"}}})).is_err());
        assert!(decode(&s,&json!({"fields":{"id":{"integerValue":"0"},"note":{"stringValue":"x".repeat(65536)}}})).is_err());
        assert!(cursor_value(&json!({"fields":{"wrong":{},"next":{"integerValue":"0"},"certificate":{"stringValue":""}}})).is_err());
    }
}
