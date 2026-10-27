use super::*;
use crate::selection::{Answers, ChoiceAnswer, Usage};
use std::collections::BTreeMap;

fn setup() -> Result<(Schema, Schema, ChoiceSet, Evidence)> {
    let source = Schema {
        version: 1,
        namespace: "src".into(),
        relation: "orders".into(),
        fields: vec![
            Field {
                name: "id".into(),
                data_type: DataType::I32,
                nullable: false,
            },
            Field {
                name: "a".into(),
                data_type: DataType::Utf8,
                nullable: true,
            },
            Field {
                name: "b".into(),
                data_type: DataType::Utf8,
                nullable: true,
            },
        ],
        primary_key: vec!["id".into()],
    };
    let mut target = source.clone();
    target.namespace = "dst".into();
    target.fields[0].data_type = DataType::I64;
    let choices = ChoiceSet::generate(&source, &target)?;
    let meanings = source
        .fields
        .iter()
        .map(|f| (f.name.clone(), format!("Meaning of {}", f.name)))
        .collect();
    let evidence = Evidence {
        source_meanings: meanings,
        target_meanings: target
            .fields
            .iter()
            .map(|f| (f.name.clone(), format!("Target meaning of {}", f.name)))
            .collect(),
        context: "Synthetic schema".into(),
    };
    Ok((source, target, choices, evidence))
}
fn response(choice: &str, probs: [f64; 3]) -> Response {
    Response {
        model: MODEL.into(),
        answers: Answers {
            pick: ChoiceAnswer {
                kind: "choice".into(),
                choice: choice.into(),
                confidence: 0.95,
                probabilities: [
                    ("option_1".into(), probs[0]),
                    ("option_2".into(), probs[1]),
                    ("review".into(), probs[2]),
                ]
                .into(),
            },
        },
        usage: Usage {
            input_tokens: 300,
            output_tokens: 30,
        },
    }
}
#[test]
fn enumeration_is_complete_canonical_and_compiler_checked() -> Result<()> {
    let (mut s, mut t, choices, _) = setup()?;
    assert_eq!(choices.candidates().len(), 2);
    for c in choices.candidates().values() {
        compile(&s, &t, c)?;
    }
    s.fields.reverse();
    t.fields.reverse();
    assert_eq!(
        choices.fingerprint()?,
        ChoiceSet::generate(&s, &t)?.fingerprint()?
    );
    Ok(())
}
#[test]
fn enumeration_never_returns_an_incomplete_shortlist() -> Result<()> {
    let (mut s, mut t, _, _) = setup()?;
    for name in ["c", "d", "e"] {
        for schema in [&mut s, &mut t] {
            schema.fields.push(Field {
                name: name.into(),
                data_type: DataType::Utf8,
                nullable: true,
            });
        }
    }
    assert!(matches!(ChoiceSet::generate(&s, &t), Err(Error::Bounds(_))));
    t.fields[1].data_type = DataType::Bool;
    assert!(ChoiceSet::generate(&s, &t).is_err());
    Ok(())
}
#[test]
fn review_and_probability_gate_never_return_executable_candidates() -> Result<()> {
    let (_, _, choices, evidence) = setup()?;
    let request = SelectionRequest::new(choices, evidence)?;
    for r in [
        response("review", [0.01, 0.01, 0.98]),
        response("option_1", [0.6, 0.39, 0.01]),
    ] {
        assert!(matches!(request.decide(&r)?, Decision::Review { .. }));
        assert_eq!(request.selected(&r)?, None);
    }
    let r = response("option_2", [0.01, 0.98, 0.01]);
    assert!(matches!(request.decide(&r)?, Decision::Accepted { .. }));
    assert!(request.selected(&r)?.is_some());
    Ok(())
}
#[test]
fn malformed_or_foreign_responses_fail_closed() -> Result<()> {
    let (_, _, choices, evidence) = setup()?;
    let request = SelectionRequest::new(choices, evidence)?;
    let good = response("option_1", [0.98, 0.01, 0.01]);
    let mut r = good.clone();
    r.model = "jev-latest".into();
    assert!(request.decide(&r).is_err());
    let mut r = good.clone();
    r.answers.pick.kind = "noul".into();
    assert!(request.decide(&r).is_err());
    let mut r = good.clone();
    r.answers.pick.choice = "injected".into();
    assert!(request.decide(&r).is_err());
    let mut r = good.clone();
    r.answers.pick.probabilities.remove("review");
    assert!(request.decide(&r).is_err());
    let mut r = good.clone();
    r.answers.pick.probabilities.insert("extra".into(), 0.0);
    assert!(request.decide(&r).is_err());
    for bad in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        let mut r = good.clone();
        r.answers.pick.probabilities.insert("option_1".into(), bad);
        assert!(request.decide(&r).is_err());
    }
    assert!(
        request
            .decide(&response("option_1", [0.3, 0.1, 0.1]))
            .is_err()
    );
    assert!(
        request
            .decide(&response("option_1", [0.01, 0.98, 0.01]))
            .is_err()
    );
    let mut r = good;
    r.usage.input_tokens = 20_001;
    assert!(request.decide(&r).is_err());
    Ok(())
}
#[test]
fn evidence_and_schema_changes_bind_a_different_request() -> Result<()> {
    let (s, mut t, choices, mut evidence) = setup()?;
    let original = SelectionRequest::new(choices.clone(), evidence.clone())?;
    evidence.context.push_str(" Role meanings corrected.");
    assert_ne!(
        original.fingerprint()?,
        SelectionRequest::new(choices.clone(), evidence.clone())?.fingerprint()?
    );
    t.relation = "changed".into();
    assert_ne!(
        original.fingerprint()?,
        SelectionRequest::new(ChoiceSet::generate(&s, &t)?, evidence.clone())?.fingerprint()?
    );
    evidence.source_meanings = BTreeMap::new();
    assert!(SelectionRequest::new(choices, evidence).is_err());
    Ok(())
}
#[test]
fn response_wire_shape_rejects_injected_fields()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut v = serde_json::to_value(response("option_1", [0.98, 0.01, 0.01]))?;
    v["answers"]["pick"]["sql"] = serde_json::json!("DROP TABLE destination");
    assert!(serde_json::from_value::<Response>(v).is_err());
    Ok(())
}
