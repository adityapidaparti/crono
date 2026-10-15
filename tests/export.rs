use chrono::NaiveDate;
use crono::client::build_diary_export;
use serde_json::json;

#[test]
fn export_retains_nonfood_entries_recipe_quantity_and_unknown_nutrients() {
    let day = NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
    let diary = json!({"diary":[
        {"type":"Serving","foodId":42,"grams":2.5,"future_field":"kept"},
        {"type":"Exercise","id":9,"duration":30}
    ],"future_metadata":true});
    let foods = json!({"foods":[{"id":42,"measures":[{"type":"Recipe"}],
        "nutrients":[{"id":999999,"amount":123}]}]});
    let value = build_diary_export(day, 7, "America/New_York", diary.clone(), foods.clone()).unwrap();
    assert_eq!(value["diary_response"], diary);
    assert_eq!(value["foods_response"], foods);
    assert_eq!(value["account"]["user_id"], 7);
    assert_eq!(value["missing_food_ids"], json!([]));
}

#[test]
fn missing_metadata_is_explicit_and_malformed_responses_fail() {
    let day = NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
    let diary = json!({"diary":[{"type":"Serving","foodId":42}]});
    let value = build_diary_export(day, 7, "UTC", diary, json!({"foods":[]})).unwrap();
    assert_eq!(value["missing_food_ids"], json!([42]));
    assert!(build_diary_export(day, 7, "UTC", json!({}), json!({"foods":[]})).is_err());
    assert!(build_diary_export(day, 7, "UTC", json!({"diary":[]}), json!({})).is_err());
}
