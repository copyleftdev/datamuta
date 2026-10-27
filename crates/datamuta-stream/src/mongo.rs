//! Loopback replica-set laboratory publisher. Flat typed BSON only. Transactions
//! atomically publish documents, a receipt and the compare-and-swap frontier.
use crate::{
    publisher::{Boundary, Cursor, cert, check_cursor},
    *,
};
use datamuta_plan::{DataType, Record, Scalar};
use mongodb::{
    Client, ClientSession, Database,
    bson::{Bson, Document, doc},
    options::{
        Acknowledgment, ClientOptions, ReadConcern, ReadPreference, SelectionCriteria, WriteConcern,
    },
};
use spool::Meta;
use std::time::Duration;
pub async fn connect(endpoint: &Endpoint) -> Result<(Client, Database)> {
    endpoint.validate()?;
    require(
        endpoint.lab_only && endpoint.verified.is_none(),
        "MongoDB requires loopback lab profile",
    )?;
    let mut options = ClientOptions::parse(format!(
        "mongodb://127.0.0.1:{}/?directConnection=true&retryWrites=false",
        endpoint.port
    ))
    .await?;
    options.server_selection_timeout = Some(Duration::from_secs(5));
    options.connect_timeout = Some(Duration::from_secs(5));
    options.selection_criteria = Some(SelectionCriteria::ReadPreference(ReadPreference::Primary));
    options.read_concern = Some(ReadConcern::majority());
    let client = Client::with_options(options)?;
    let db = client.database(&endpoint.namespace);
    let admin = client.database("admin");
    let hello = admin.run_command(doc! {"hello":1}).await?;
    require(
        hello.get_str("setName").is_ok() && hello.get_bool("isWritablePrimary").ok() == Some(true),
        "MongoDB writable replica-set primary required",
    )?;
    let info = admin.run_command(doc! {"buildInfo":1}).await?;
    require(
        info.get_str("version").map_err(err)?.starts_with("8.0."),
        "MongoDB version outside admitted profile",
    )?;
    Ok((client, db))
}
/// Exact native validator used by provisioning and independently checked on open.
/// `_id` is the JSON encoding of the ordered signed integer identity tuple.
pub fn validator(schema: &Schema) -> Result<Document> {
    schema.validate()?;
    require(schema.field("_id").is_none(), "MongoDB _id is reserved")?;
    let mut properties = doc! {"_id":{"bsonType":"string"}};
    let mut required = vec![Bson::String("_id".into())];
    for f in &schema.fields {
        let ty = match f.data_type {
            DataType::Bool => "bool",
            DataType::I32 => "int",
            DataType::I64 => "long",
            DataType::Utf8 => "string",
        };
        let types = if f.nullable {
            vec![Bson::String(ty.into()), Bson::String("null".into())]
        } else {
            vec![Bson::String(ty.into())]
        };
        properties.insert(&f.name, doc! {"bsonType":types});
        required.push(Bson::String(f.name.clone()));
    }
    // Field order is not semantic. Keep validators stable across normalized plans.
    required.sort_by_key(|v| v.as_str().unwrap_or_default().to_owned());
    let mut sorted = Document::new();
    let mut keys = properties.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    for key in keys {
        if let Some(v) = properties.remove(&key) {
            sorted.insert(key, v);
        }
    }
    Ok(
        doc! {"$jsonSchema":{"bsonType":"object","required":required,"additionalProperties":false,"properties":sorted}},
    )
}
pub fn metadata_validator(cursor: bool) -> Document {
    if cursor {
        doc! {"$jsonSchema":{"bsonType":"object","required":["_id","plan","next","certificate"],"additionalProperties":false,"properties":{"_id":{"enum":[1]},"plan":{"bsonType":"string"},"next":{"bsonType":"long","minimum":0},"certificate":{"bsonType":"string"}}}}
    } else {
        doc! {"$jsonSchema":{"bsonType":"object","required":["_id","hash"],"additionalProperties":false,"properties":{"_id":{"bsonType":"long","minimum":0},"hash":{"bsonType":"string"}}}}
    }
}
async fn check_collection(db: &Database, name: &str, expected: Document) -> Result<()> {
    let result = db
        .run_command(doc! {"listCollections":1,"filter":{"name":name},"cursor":{"batchSize":2}})
        .await?;
    let cursor = result.get_document("cursor").map_err(err)?;
    require(
        cursor.get_i64("id").map_err(err)? == 0,
        "MongoDB metadata cursor exceeds bound",
    )?;
    let batch = cursor.get_array("firstBatch").map_err(err)?;
    require(batch.len() == 1, "MongoDB collection missing")?;
    let info = batch[0]
        .as_document()
        .ok_or_else(|| err("MongoDB collection metadata"))?;
    require(
        info.get_str("type").ok() == Some("collection"),
        "MongoDB collection type",
    )?;
    let options = info.get_document("options").map_err(err)?;
    require(
        *options
            == doc! {"validator":expected,"validationLevel":"strict","validationAction":"error"},
        "MongoDB validator/options differ from compiled mapping",
    )?;
    let indexes = db
        .run_command(doc! {"listIndexes":name,"cursor":{"batchSize":2}})
        .await?;
    let cursor = indexes.get_document("cursor").map_err(err)?;
    let batch = cursor.get_array("firstBatch").map_err(err)?;
    require(
        cursor.get_i64("id").map_err(err)? == 0 && batch.len() == 1,
        "MongoDB additional indexes refused",
    )?;
    let index = batch[0]
        .as_document()
        .ok_or_else(|| err("MongoDB index metadata"))?;
    require(
        index.get_document("key").map_err(err)? == &doc! {"_id":1}
            && index.get_str("name").ok() == Some("_id_"),
        "MongoDB identity index differs",
    )
}
async fn admit(db: &Database, s: &Schema) -> Result<()> {
    check_collection(db, &s.relation, validator(s)?).await?;
    check_collection(db, "stream_cursor", metadata_validator(true)).await?;
    check_collection(db, "stream_receipts", metadata_validator(false)).await
}
pub(crate) async fn admit_config(config: &Config) -> Result<()> {
    let (_c, db) = connect(&config.target_endpoint).await?;
    admit(&db, &config.target).await
}
pub struct Destination {
    client: Client,
    db: Database,
    mapping: CompiledMapping,
    plan: Fingerprint,
    partitions: u64,
}
impl Destination {
    pub async fn open(p: &Plan) -> Result<Self> {
        let mapping = p.validate()?;
        let (client, db) = connect(&p.config.target_endpoint).await?;
        admit(&db, mapping.target()).await?;
        Ok(Self {
            client,
            db,
            mapping,
            plan: p.fingerprint()?,
            partitions: p.snapshot.partitions,
        })
    }
    fn decode_cursor(&self, d: Document) -> Result<Cursor> {
        require(
            d.get_i32("_id").ok() == Some(1),
            "stream cursor singleton missing",
        )?;
        let next = d.get_i64("next").map_err(err)?;
        require(next >= 0, "stream frontier bounds")?;
        check_cursor(
            Cursor {
                next: next as u64,
                plan: d.get_str("plan").map_err(err)?.into(),
                certificate: d.get_str("certificate").map_err(err)?.into(),
            },
            &self.plan,
            self.partitions,
        )
    }
    pub(crate) async fn cursor(&self) -> Result<Cursor> {
        let d = self
            .db
            .collection::<Document>("stream_cursor")
            .find_one(doc! {"_id":1})
            .await?
            .ok_or_else(|| err("stream cursor singleton missing"))?;
        self.decode_cursor(d)
    }
    pub(crate) async fn receipt(&self, i: u64) -> Result<Option<String>> {
        self.db
            .collection::<Document>("stream_receipts")
            .find_one(doc! {"_id":i as i64})
            .await?
            .map(|d| d.get_str("hash").map(str::to_owned).map_err(err))
            .transpose()
    }
    pub(crate) async fn apply(
        &mut self,
        m: &Meta,
        records: &[Record],
        observe: &mut impl FnMut(Boundary, u64) -> Result<()>,
    ) -> Result<bool> {
        let mut session = self.client.start_session().await?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(
                WriteConcern::builder()
                    .w(Acknowledgment::Majority)
                    .journal(true)
                    .build(),
            )
            .max_commit_time(Duration::from_secs(10))
            .await?;
        let result = self.transaction(&mut session, m, records, observe).await;
        if result.is_err() {
            let _ = session.abort_transaction().await;
        }
        result
    }
    async fn transaction(
        &self,
        session: &mut ClientSession,
        m: &Meta,
        records: &[Record],
        observe: &mut impl FnMut(Boundary, u64) -> Result<()>,
    ) -> Result<bool> {
        let units = self.mapping.prepare(records)?;
        require(units.len() as u64 == m.rows, "partition rows differ")?;
        let cursor = self.db.collection::<Document>("stream_cursor");
        let receipts = self.db.collection::<Document>("stream_receipts");
        let state = self.decode_cursor(
            cursor
                .find_one(doc! {"_id":1})
                .session(&mut *session)
                .await?
                .ok_or_else(|| err("stream cursor singleton missing"))?,
        )?;
        let expected = cert(&self.plan, m)?;
        let receipt = receipts
            .find_one(doc! {"_id":m.index as i64})
            .session(&mut *session)
            .await?;
        if state.next > m.index {
            require(
                receipt.as_ref().and_then(|d| d.get_str("hash").ok()) == Some(expected.as_str()),
                "replayed partition receipt conflict",
            )?;
            session.commit_transaction().await?;
            return Ok(false);
        }
        require(state.next == m.index, "noncontiguous partition attempt")?;
        require(receipt.is_none(), "receipt exists ahead of atomic cursor")?;
        // This write conflicts with concurrent publishers before any document is
        // inserted. Unknown commit results return an error; fresh resume resolves.
        let changed=cursor.update_one(doc!{"_id":1,"next":state.next as i64,"plan":state.plan,"certificate":state.certificate},doc!{"$set":{"plan":self.plan.as_str(),"next":(m.index+1) as i64,"certificate":expected.as_str()}}).session(&mut *session).await?;
        require(
            changed.modified_count == 1,
            "atomic cursor compare-and-swap failed",
        )?;
        let mut docs = Vec::with_capacity(units.len());
        for u in units {
            let identity = self
                .mapping
                .target()
                .primary_key
                .iter()
                .map(|k| match u.fields().get(k) {
                    Some(Scalar::I32(v)) => Ok(i64::from(*v)),
                    Some(Scalar::I64(v)) => Ok(*v),
                    _ => Err(err("invalid MongoDB identity")),
                })
                .collect::<Result<Vec<_>>>()?;
            let mut d = doc! {"_id":serde_json::to_string(&identity)?};
            for (k, v) in u.fields() {
                d.insert(
                    k,
                    match v {
                        Scalar::Bool(v) => Bson::Boolean(*v),
                        Scalar::I32(v) => Bson::Int32(*v),
                        Scalar::I64(v) => Bson::Int64(*v),
                        Scalar::Utf8(v) => Bson::String(v.clone()),
                        Scalar::Null => Bson::Null,
                    },
                );
            }
            docs.push(d);
        }
        self.db
            .collection::<Document>(&self.mapping.target().relation)
            .insert_many(docs)
            .session(&mut *session)
            .await?;
        receipts
            .insert_one(doc! {"_id":m.index as i64,"hash":expected.as_str()})
            .session(&mut *session)
            .await?;
        admit(&self.db, self.mapping.target()).await?;
        observe(Boundary::BeforeCommit, m.index)?;
        session.commit_transaction().await?;
        observe(Boundary::AfterCommit, m.index)?;
        Ok(true)
    }
}
