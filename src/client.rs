//! HTTP client for Cronometer's reverse-engineered mobile REST API
//! (`mobile.cronometer.com`, the API the Android app speaks).
//!
//! Protocol notes (verified against the app's traffic):
//! - v2: everything is `POST /api/v2/<endpoint>`; auth rides *inside* the JSON
//!   body as an `auth` object; content-type is `text/plain; charset=utf-8`;
//!   failures often arrive as HTTP 200 with `{"result": "FAIL", ...}`.
//! - v3: RESTful, auth via `x-crono-session` header; only used for deletes.
//! - Dates are non-zero-padded `YYYY-M-D`, computed in the *account's*
//!   timezone (returned at login), not the host clock.
//! - Login MUST send `"timezone": null` — a non-null value permanently
//!   overwrites the account's timezone server-side.
//! - Logins are rate-limited per account, so the session token is cached on
//!   disk and re-login happens only when the token is rejected.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{NaiveDate, NaiveTime};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::session::{self, Credentials, Session};
use crate::types::{cron_day, cron_time, Amount, Meal};

const BASE: &str = "https://mobile.cronometer.com";
const APP_VERSION: &str = "4.48.2 b2807-a";
const APP_BUILD: &str = "2807";

#[derive(Debug, Clone)]
pub struct AuthState {
    pub user_id: i64,
    pub token: String,
    pub timezone: String,
}

pub struct Client {
    http: reqwest::blocking::Client,
    auth: Option<AuthState>,
}

// ---------- response types ----------

#[derive(Debug, Deserialize)]
pub struct SearchFood {
    pub id: i64,
    pub name: String,
    #[serde(rename = "measureId", default)]
    pub measure_id: i64,
    #[serde(rename = "measureDisplayName", default)]
    pub measure_display_name: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct Measure {
    pub id: i64,
    pub name: String,
    /// Grams per one unit of this measure (serving count for Recipe measures).
    pub value: f64,
    #[serde(default)]
    pub amount: f64,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub struct NutrientAmount {
    pub id: i64,
    pub amount: f64,
}

#[derive(Debug, Deserialize)]
pub struct Food {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(rename = "defaultMeasureId", default)]
    pub default_measure_id: i64,
    #[serde(default)]
    pub measures: Vec<Measure>,
    /// Per-100g, except Recipe-measure foods (per reference serving).
    #[serde(default)]
    pub nutrients: Vec<NutrientAmount>,
}

pub const NUTRIENT_ENERGY_KCAL: i64 = 208;
pub const NUTRIENT_PROTEIN: i64 = 203;
pub const NUTRIENT_FAT: i64 = 204;
pub const NUTRIENT_CARBS: i64 = 205;

impl Food {
    pub fn nutrient(&self, id: i64) -> Option<f64> {
        self.nutrients.iter().find(|n| n.id == id).map(|n| n.amount)
    }

    pub fn measure(&self, id: i64) -> Option<&Measure> {
        self.measures.iter().find(|m| m.id == id)
    }

    pub fn default_measure(&self) -> Option<&Measure> {
        self.measure(self.default_measure_id).or(self.measures.first())
    }
}

/// A resolved "how much": which measure to reference and the literal grams
/// (or serving count, for Recipe measures) to submit.
#[derive(Debug)]
pub struct ResolvedAmount {
    pub measure_id: i64,
    pub grams: f64,
    pub display: String,
    pub is_recipe: bool,
}

// ---------- client ----------

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent("Dart/3.9 (dart:io)")
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let auth = session::load_session()?.map(|s: Session| AuthState {
            user_id: s.user_id.unwrap_or_default(),
            token: s.token,
            timezone: s.timezone.unwrap_or_else(|| "UTC".into()),
        });
        Ok(Client { http, auth })
    }

    pub fn is_authenticated(&self) -> bool {
        self.auth.is_some()
    }

    /// Export with the existing session only. Never retry a login or switch to
    /// saved credentials while collecting an account's historical data.
    pub fn export_day(&self, day: NaiveDate) -> Result<Value> {
        let a = self.auth()?;
        let saved = session::load_session()?.context("export requires an existing session")?;
        let timezone = saved.timezone.as_deref().context("session has no account timezone; log in again")?;
        timezone.parse::<chrono_tz::Tz>().context("invalid account timezone")?;
        if saved.user_id != Some(a.user_id) || a.user_id <= 0 || saved.token != a.token || a.token.is_empty() {
            bail!("session identity changed or is incomplete; log in again before exporting");
        }
        let diary = self.export_read("get_diary", json!({"day": cron_day(day)}))?;
        let entries = diary.get("diary").and_then(Value::as_array)
            .context("get_diary response missing diary array; refusing an empty-data fallback")?;
        let mut ids = std::collections::BTreeSet::new();
        for entry in entries {
            if entry.get("type").and_then(Value::as_str) == Some("Serving") {
                ids.insert(entry.get("foodId").and_then(Value::as_i64)
                    .context("Serving is missing foodId")?);
            }
        }
        let foods = if ids.is_empty() { json!({"foods": []}) } else {
            self.export_read("get_foods", json!({"ids": ids}))?
        };
        build_diary_export(day, a.user_id, timezone, diary, foods)
    }

    fn export_read(&self, endpoint: &str, mut body: Value) -> Result<Value> {
        let a = self.auth()?;
        body["auth"] = json!({"userId": a.user_id, "token": a.token, "api": 3,
            "os": "Android", "build": APP_BUILD, "flavour": "free"});
        body["lastSeen"] = json!(0);
        body["config"] = json!({"call_version": 1});
        let response = self.http.post(format!("{BASE}/api/v2/{endpoint}"))
            .header("content-type", "text/plain; charset=utf-8")
            .body(serde_json::to_string(&body)?)
            .send().map_err(|_| anyhow!("Cronometer export network request failed"))?;
        if !response.status().is_success() {
            bail!("Cronometer export HTTP {}; no automatic reauthentication", response.status());
        }
        let value: Value = response.json().map_err(|_| anyhow!("Cronometer export returned invalid JSON"))?;
        if Self::is_auth_failure(&value) {
            bail!("Cronometer rejected export; verify the session with a fresh login (no automatic retry)");
        }
        Ok(value)
    }

    /// "Now" in the account's timezone (login response), falling back to local.
    pub fn account_now(&self) -> (NaiveDate, NaiveTime) {
        let tz: Option<chrono_tz::Tz> = self
            .auth
            .as_ref()
            .and_then(|a| a.timezone.parse().ok());
        match tz {
            Some(tz) => {
                let now = chrono::Utc::now().with_timezone(&tz);
                (now.date_naive(), now.time())
            }
            None => {
                let now = chrono::Local::now();
                (now.date_naive(), now.time())
            }
        }
    }

    // ---------- auth ----------

    pub fn login(&mut self, email: &str, password: &str) -> Result<AuthState> {
        let body = json!({
            "email": email,
            "password": password,
            // MUST be null: a non-null value overwrites the account timezone.
            "timezone": null,
            "userCode": null,
            "build": APP_VERSION,
            "device": "Android 14 (SDK 34), Google Pixel 6 Pro",
            "firebaseToken": "",
            "features": {
                "food_search_config": "{\"newSearch\": true, \"newSpellcheck\": true}",
                "use_gpt_autofill": "true"
            },
            "auth": {"userId": null, "token": null, "api": 3, "os": "Android",
                     "build": APP_BUILD, "flavour": "free"},
            "lastSeen": 0,
            "config": {"call_version": 2}
        });
        let resp: Value = self.raw_v2_post("login", &body)?;
        let session_key = resp.get("sessionKey").and_then(Value::as_str);
        let ok = resp.get("result").and_then(Value::as_str) == Some("SUCCESS");
        let (Some(token), true) = (session_key, ok || session_key.is_some()) else {
            let err = resp.get("error").and_then(Value::as_str).unwrap_or("login failed");
            bail!("login failed: {err}");
        };
        let state = AuthState {
            user_id: resp
                .get("id")
                .and_then(Value::as_i64)
                .context("login response missing user id")?,
            token: token.to_string(),
            timezone: resp
                .get("timezone")
                .and_then(Value::as_str)
                .unwrap_or("UTC")
                .to_string(),
        };
        session::save_session(&Session {
            token: state.token.clone(),
            user_id: Some(state.user_id),
            timezone: Some(state.timezone.clone()),
            saved_at: Some(chrono::Utc::now().to_rfc3339()),
        })?;
        self.auth = Some(state.clone());
        Ok(state)
    }

    fn relogin_from_saved_credentials(&mut self) -> Result<()> {
        let creds: Credentials = session::load_credentials()?
            .context("session expired and no saved credentials — run `crono login <email> --remember`")?;
        self.login(&creds.username, &creds.password)?;
        Ok(())
    }

    fn auth(&self) -> Result<&AuthState> {
        self.auth
            .as_ref()
            .ok_or_else(|| anyhow!("not logged in — run `crono login <email>`"))
    }

    // ---------- transport ----------

    fn raw_v2_post(&self, endpoint: &str, body: &Value) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{BASE}/api/v2/{endpoint}"))
            .header("content-type", "text/plain; charset=utf-8")
            .body(serde_json::to_string(body)?)
            .send()
            .with_context(|| format!("POST /api/v2/{endpoint}"))?;
        let status = resp.status();
        if status.as_u16() == 429 {
            bail!("rate limited by Cronometer (HTTP 429) — wait a few minutes");
        }
        let text = resp.text()?;
        if !status.is_success() && status.as_u16() != 401 && status.as_u16() != 403 {
            bail!("/api/v2/{endpoint} failed: HTTP {status}: {text}");
        }
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("/api/v2/{endpoint}: non-JSON response: {text}"))?;
        if !status.is_success() {
            // 401/403 → surface as auth error for the retry layer.
            bail!("AUTH_FAILED: HTTP {status}");
        }
        Ok(value)
    }

    fn is_auth_failure(value: &Value) -> bool {
        matches!(
            value.get("result").and_then(Value::as_str),
            Some("FAIL") | Some("FAILURE")
        )
    }

    /// Authenticated v2 call: injects `auth`/`lastSeen`/`config.call_version`,
    /// re-logs-in and retries exactly once if the token is rejected.
    fn v2(&mut self, endpoint: &str, mut body: Value, call_version: u32) -> Result<Value> {
        for attempt in 0..2 {
            let a = self.auth()?;
            let obj = body
                .as_object_mut()
                .ok_or_else(|| anyhow!("v2 body must be a JSON object"))?;
            obj.insert(
                "auth".into(),
                json!({"userId": a.user_id, "token": a.token, "api": 3,
                       "os": "Android", "build": APP_BUILD, "flavour": "free"}),
            );
            obj.insert("lastSeen".into(), json!(0));
            obj.entry("config")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap()
                .insert("call_version".into(), json!(call_version));

            let auth_failed = match self.raw_v2_post(endpoint, &body) {
                Ok(v) if Self::is_auth_failure(&v) => {
                    let err = v.get("error").and_then(Value::as_str).unwrap_or("request failed");
                    if attempt == 1 || !err.to_lowercase().contains("auth") {
                        bail!("/api/v2/{endpoint}: {err}");
                    }
                    true
                }
                Ok(v) => return Ok(v),
                Err(e) if e.to_string().starts_with("AUTH_FAILED") && attempt == 0 => true,
                Err(e) => return Err(e),
            };
            if auth_failed {
                session::clear_session()?;
                self.auth = None;
                self.relogin_from_saved_credentials()?;
            }
        }
        unreachable!("v2 retry loop always returns or bails")
    }

    // ---------- endpoints ----------

    pub fn search(&mut self, query: &str) -> Result<Vec<SearchFood>> {
        let resp = self.v2(
            "find_food",
            json!({
                "query": query,
                "tab": "ALL",
                "sources": ["All"],
                "config": {"newSearch": true, "newSpellcheck": true}
            }),
            1,
        )?;
        let foods = resp
            .get("foods")
            .cloned()
            .unwrap_or_else(|| json!([]));
        Ok(serde_json::from_value(foods).context("parsing find_food response")?)
    }

    pub fn get_food(&mut self, id: i64) -> Result<Food> {
        let resp = self.v2("get_food", json!({"id": id}), 1)?;
        Ok(serde_json::from_value(resp).context("parsing get_food response")?)
    }

    pub fn get_foods(&mut self, ids: &[i64]) -> Result<Vec<Food>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let resp = self.v2("get_foods", json!({"ids": ids}), 1)?;
        let foods = resp.get("foods").cloned().unwrap_or_else(|| json!([]));
        Ok(serde_json::from_value(foods).context("parsing get_foods response")?)
    }

    pub fn add_serving(
        &mut self,
        food_id: i64,
        resolved: &ResolvedAmount,
        meal: Meal,
        day: NaiveDate,
        time: NaiveTime,
    ) -> Result<Value> {
        let user_id = self.auth()?.user_id;
        self.v2(
            "add_serving",
            json!({
                "serving": {
                    "order": meal.pack_order(),
                    "day": cron_day(day),
                    "time": cron_time(time),
                    "offset": null,
                    "source": null,
                    "userId": user_id,
                    "servingId": null,
                    "type": "Serving",
                    "foodId": food_id,
                    "measureId": resolved.measure_id,
                    "grams": resolved.grams,
                    "translationId": 0
                }
            }),
            2,
        )
    }

    /// Raw diary entries for a day (all types, as returned — deletes need the
    /// complete objects round-tripped).
    pub fn get_diary(&mut self, day: NaiveDate) -> Result<Vec<Value>> {
        let resp = self.v2("get_diary", json!({"day": cron_day(day)}), 1)?;
        Ok(resp
            .get("diary")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Delete diary entries: v3 endpoint, full entry objects required.
    pub fn delete_entries(&mut self, entries: &[Value]) -> Result<()> {
        let a = self.auth()?;
        let resp = self
            .http
            .delete(format!("{BASE}/api/v3/user/{}/diary-entries", a.user_id))
            .header("x-crono-session", &a.token)
            .header("x-crono-app-os", "android")
            .header("x-crono-app-build-number", APP_BUILD)
            .header("x-crono-app-version", "4.48.2")
            .header("content-type", "application/json; charset=utf-8")
            .json(&json!({"diaryEntries": entries}))
            .send()
            .context("DELETE diary-entries")?;
        let status = resp.status();
        if status.as_u16() == 204 || status.is_success() {
            Ok(())
        } else {
            bail!("delete failed: HTTP {status}: {}", resp.text().unwrap_or_default())
        }
    }
}

/// Keep original diary/food objects and unknown fields, with explicit identity.
/// Quantities are not converted here: Recipe `grams` can mean serving counts.
pub fn build_diary_export(day: NaiveDate, user_id: i64, timezone: &str, diary: Value, foods: Value) -> Result<Value> {
    let entries = diary.get("diary").and_then(Value::as_array).context("missing diary array")?;
    let food_rows = foods.get("foods").and_then(Value::as_array).context("missing foods array")?;
    if entries.iter().any(|e| !e.is_object()) || food_rows.iter().any(|f| !f.is_object()) {
        bail!("export contains malformed records");
    }
    let present: std::collections::BTreeSet<i64> = food_rows.iter().filter_map(|f| f.get("id").and_then(Value::as_i64)).collect();
    let missing: std::collections::BTreeSet<i64> = entries.iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("Serving"))
        .filter_map(|e| e.get("foodId").and_then(Value::as_i64))
        .filter(|id| !present.contains(id)).collect();
    Ok(json!({"schema_version": 1, "source": "cronometer", "day": day.to_string(),
        "account": {"user_id": user_id, "timezone": timezone},
        "missing_food_ids": missing, "diary_response": diary, "foods_response": foods}))
}

// ---------- amount resolution (pure; property-tested) ----------

const GRAMS_PER_OZ: f64 = 28.349_523_125;

/// Match a parsed CLI amount against a food's measures.
pub fn resolve_amount(food: &Food, amount: &Amount) -> Result<ResolvedAmount> {
    let default = food
        .default_measure()
        .ok_or_else(|| anyhow!("food '{}' has no measures", food.name))?;

    let build = |m: &Measure, count: f64| {
        let is_recipe = m.kind == "Recipe";
        ResolvedAmount {
            measure_id: m.id,
            grams: if is_recipe { count * m.amount.max(1.0) } else { count * m.value },
            display: format!("{count} × {}", m.name),
            is_recipe,
        }
    };

    let Some(unit) = &amount.unit else {
        return Ok(build(default, amount.value));
    };

    // Literal weights bypass measure math: `grams` is grams.
    if matches!(unit.as_str(), "g" | "gram" | "grams") {
        let measure = find_measure(food, "g").unwrap_or(default);
        return Ok(ResolvedAmount {
            measure_id: measure.id,
            grams: amount.value,
            display: format!("{} g", amount.value),
            is_recipe: false,
        });
    }
    if matches!(unit.as_str(), "oz" | "ounce" | "ounces") {
        let measure = find_measure(food, "oz").unwrap_or(default);
        return Ok(ResolvedAmount {
            measure_id: measure.id,
            grams: amount.value * GRAMS_PER_OZ,
            display: format!("{} oz", amount.value),
            is_recipe: false,
        });
    }

    let m = find_measure(food, unit).ok_or_else(|| {
        anyhow!(
            "no measure matching '{unit}' for '{}'; available: {}",
            food.name,
            food.measures
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    Ok(build(m, amount.value))
}

/// Case-insensitive measure lookup: exact name, then prefix, then substring.
pub fn find_measure<'a>(food: &'a Food, unit: &str) -> Option<&'a Measure> {
    let unit = unit.to_ascii_lowercase();
    let names: Vec<String> = food
        .measures
        .iter()
        .map(|m| m.name.to_ascii_lowercase())
        .collect();
    if let Some(i) = names.iter().position(|n| *n == unit) {
        return food.measures.get(i);
    }
    if let Some(i) = names.iter().position(|n| n.starts_with(&unit)) {
        return food.measures.get(i);
    }
    names
        .iter()
        .position(|n| n.contains(&unit))
        .and_then(|i| food.measures.get(i))
}

/// Estimated kcal for a resolved amount (per-100g nutrients, or per reference
/// serving for Recipe measures).
pub fn estimate_kcal(food: &Food, resolved: &ResolvedAmount) -> Option<f64> {
    let energy = food.nutrient(NUTRIENT_ENERGY_KCAL)?;
    Some(if resolved.is_recipe {
        energy * resolved.grams
    } else {
        energy * resolved.grams / 100.0
    })
}
