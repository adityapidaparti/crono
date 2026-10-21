# crono

Fast local CLI for logging food to [Cronometer](https://cronometer.com). No
background process — it speaks Cronometer's mobile REST API directly and
authenticates on demand, caching the session token on disk (logins are
rate-limited server-side, so the token is reused until rejected).

## Install

```sh
cargo install --path .
```

## Usage

```sh
# one-time: authenticate (add --remember to store credentials, 0600, for auto re-auth)
crono login you@example.com --remember

# find foods
crono search hummus

# log food (amount: "120g", "5oz", "1.5 cup", "3/4 cup", "1 1/2 cup", "2" = default measure;
# foods measured only in recipe servings take a serving count, not a weight)
crono log hummus -a 120g -m dinner
crono log chicken thigh grilled -a 5oz -m dinner -d yesterday
crono log "trader joe's brookie" -a 6 -m snacks -d 2026-08-15

# no --meal → picked by time of day (04–09 breakfast, 10–13 lunch, 14–20 dinner, else snacks)
# --pick 2 → take the 2nd search result; or log by exact id: crono log '#12345' -a 100g

# inspect
crono day            # today's diary with per-entry kcal + total
crono day yesterday
crono food 12345     # measures + macros for a food
crono export yesterday  # JSON: all diary objects + full food/nutrient metadata

# fix mistakes (ids shown by `crono day`)
crono rm 987654 -d yesterday
```

## Notes

- Protocol reverse-engineered from the Android app (`mobile.cronometer.com`),
  ported from [rwestergren/cronometer-api-mcp](https://github.com/rwestergren/cronometer-api-mcp).
- Dates are computed in the *account's* timezone (from the login response),
  not the host clock.
- Session/config live in `~/Library/Application Support/crono/` (macOS).
- The pure core (parsers, wire formats, measure resolution, export) is covered
  by [Hegel](https://hegel.dev) property-based tests in `tests/properties.rs`;
  each test name states the behavior it checks. `cargo test` runs them (the
  first build compiles Hegel's engine, about a minute);
  `HEGEL_TEST_CASES=20000 cargo test --test properties` searches harder.
- `export` uses the current cached session only and validates its account ID
  and timezone. It never automatically logs in with saved credentials. It
  preserves recipe quantities and all returned nutrient fields without assuming
  everything is grams; `missing_food_ids` flags incomplete food metadata.
