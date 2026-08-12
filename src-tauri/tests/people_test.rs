// Person extraction (model-backed, like category_test): names mentioned in a note
// surface as `person` entities with a curated fact/relationship, and the author is
// never extracted as a person. Assertions are tolerant of model wording.
use tauri_app_lib::entities::normalize;
use tauri_app_lib::pipeline;

const CATALOG: &str =
    "- gym: workout logs. shape: {\"exercises\":[{\"name\":\"squat\",\"weight\":245}]}";

fn known() -> Vec<String> {
    vec!["gym".into()]
}

fn persons(p: &serde_json::Value) -> Vec<serde_json::Value> {
    p["entities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["type"] == serde_json::json!("person"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn cross_cutting_person_with_fact() {
    let p = pipeline::categorize(
        CATALOG,
        &known(),
        "Gym: squats 245 5x5 with Mike, he just got engaged",
        "2026-06-03",
    )
    .await
    .unwrap();
    println!("entities -> {}", p["entities"]);
    // the gym entry still exists
    assert_eq!(p["entries"][0]["category"], serde_json::json!("gym"));
    let people = persons(&p);
    let mike = people
        .iter()
        .find(|e| normalize(e["name"].as_str().unwrap_or("")) == "mike")
        .expect("Mike extracted as a person");
    let blob = mike.to_string().to_lowercase();
    assert!(
        blob.contains("engag"),
        "Mike's fact should mention the engagement: {blob}"
    );
}

#[tokio::test]
async fn author_not_extracted_as_person() {
    let p = pipeline::categorize(
        CATALOG,
        &known(),
        "felt strong today, hit a squat PR and was really proud of myself",
        "2026-06-03",
    )
    .await
    .unwrap();
    println!("entities -> {}", p["entities"]);
    for e in persons(&p) {
        let n = normalize(e["name"].as_str().unwrap_or(""));
        assert!(
            !["i", "me", "myself"].contains(&n.as_str()),
            "author must not be a person: {n}"
        );
    }
}

#[tokio::test]
async fn relationship_captured() {
    let p = pipeline::categorize(
        CATALOG,
        &known(),
        "my brother Tom called, he is moving to Austin next month",
        "2026-06-03",
    )
    .await
    .unwrap();
    println!("entities -> {}", p["entities"]);
    let tom = persons(&p)
        .into_iter()
        .find(|e| normalize(e["name"].as_str().unwrap_or("")).contains("tom"))
        .expect("Tom extracted as a person");
    let blob = tom.to_string().to_lowercase();
    assert!(
        blob.contains("brother"),
        "relationship should be captured: {blob}"
    );
}
