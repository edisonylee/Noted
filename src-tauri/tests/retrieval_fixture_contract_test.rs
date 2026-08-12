use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Fixture {
    contract_version: String,
    documents: Vec<Document>,
    query_sets: Vec<QuerySet>,
}

#[derive(Deserialize)]
struct Document {
    uri: String,
    scope: String,
    sensitivity: String,
    kind: String,
    revision: u64,
    deleted: bool,
    content_hash: String,
    body: String,
}

#[derive(Deserialize)]
struct QuerySet {
    id: String,
    class: String,
    allowed_scopes: Vec<String>,
    allowed_kinds: Vec<String>,
    answerability: String,
    expected_evidence: Vec<Evidence>,
    forbidden_resources: Vec<String>,
    tuning_queries: Vec<String>,
    held_out_queries: Vec<String>,
}

#[derive(Deserialize)]
struct Evidence {
    uri: String,
    start_byte: usize,
    end_byte: usize,
}

#[test]
fn retrieval_fixture_meets_the_phase0_contract() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/agent_context/retrieval_v1.json"))
            .expect("valid retrieval fixture JSON");
    assert_eq!(fixture.contract_version, "noted.retrieval-fixture.v1");

    let documents: HashMap<&str, &Document> = fixture
        .documents
        .iter()
        .map(|document| (document.uri.as_str(), document))
        .collect();
    assert_eq!(
        documents.len(),
        fixture.documents.len(),
        "document URIs are unique"
    );
    for document in &fixture.documents {
        assert!(document.uri.starts_with("noted://library/"));
        assert!(document.revision > 0);
        assert!(!document.scope.is_empty());
        assert!(!document.sensitivity.is_empty());
        assert!(!document.kind.is_empty());
        let actual_hash = format!("{:x}", Sha256::digest(document.body.as_bytes()));
        assert_eq!(
            actual_hash, document.content_hash,
            "hash for {}",
            document.uri
        );
    }

    let required_classes: HashSet<&str> = [
        "exact",
        "semantic",
        "temporal",
        "transcript",
        "relationship",
        "broad_theme",
        "negative",
        "permission",
        "lifecycle",
        "multi_hop",
    ]
    .into_iter()
    .collect();
    let mut questions_per_class: HashMap<&str, usize> = HashMap::new();
    let mut total_questions = 0_usize;
    let mut held_out_questions = 0_usize;
    let mut question_texts = HashSet::new();

    for set in &fixture.query_sets {
        assert!(
            required_classes.contains(set.class.as_str()),
            "class for {}",
            set.id
        );
        assert!(!set.allowed_scopes.is_empty(), "scope for {}", set.id);
        assert!(!set.allowed_kinds.is_empty(), "kind for {}", set.id);
        assert!(matches!(
            set.answerability.as_str(),
            "answer" | "no_answer" | "deny"
        ));
        if set.answerability == "answer" {
            assert!(!set.expected_evidence.is_empty(), "evidence for {}", set.id);
        } else {
            assert!(
                set.expected_evidence.is_empty(),
                "non-answer evidence for {}",
                set.id
            );
        }

        for evidence in &set.expected_evidence {
            let document = documents
                .get(evidence.uri.as_str())
                .unwrap_or_else(|| panic!("missing evidence resource for {}", set.id));
            assert!(!document.deleted, "deleted evidence for {}", set.id);
            assert!(
                set.allowed_scopes.contains(&document.scope),
                "scope leak for {}",
                set.id
            );
            assert!(
                set.allowed_kinds.contains(&document.kind),
                "kind leak for {}",
                set.id
            );
            assert!(evidence.start_byte < evidence.end_byte);
            assert!(evidence.end_byte <= document.body.len());
            assert!(document.body.is_char_boundary(evidence.start_byte));
            assert!(document.body.is_char_boundary(evidence.end_byte));
        }
        for uri in &set.forbidden_resources {
            let document = documents
                .get(uri.as_str())
                .unwrap_or_else(|| panic!("missing forbidden resource for {}", set.id));
            assert!(
                document.deleted
                    || !set.allowed_scopes.contains(&document.scope)
                    || document.sensitivity == "restricted",
                "forbidden resource needs a lifecycle, scope, or sensitivity reason"
            );
        }

        for query in set.tuning_queries.iter().chain(&set.held_out_queries) {
            assert!(!query.trim().is_empty());
            assert!(question_texts.insert(query), "duplicate query: {query}");
        }
        let set_total = set.tuning_queries.len() + set.held_out_queries.len();
        total_questions += set_total;
        held_out_questions += set.held_out_queries.len();
        *questions_per_class.entry(&set.class).or_default() += set_total;
    }

    assert_eq!(total_questions, 150);
    assert!(
        held_out_questions * 5 >= total_questions,
        "at least 20% held out"
    );
    for class in required_classes {
        assert!(
            questions_per_class.get(class).copied().unwrap_or_default() >= 15,
            "at least 15 questions for {class}"
        );
    }
}
