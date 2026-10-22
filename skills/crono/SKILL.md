---
name: crono
description: Log food to the user's Cronometer diary and read it back with the `crono` CLI. Search foods, log a portion to a meal and day, show a day's diary with calories, remove a wrong entry, and export a day as JSON. Use when the user says what they ate, asks what they've logged or how many calories they've had, or wants a diary entry fixed.
---

# Cronometer food logging with crono

`crono` is on PATH. `log` and `rm` change the user's real Cronometer diary. `search`, `food`, `day` and `export` only read.

## First check

Run `crono day`. If it fails with a login or session error, such as "session expired and no saved credentials", ask the user to run `crono login <their email> --remember` in their own terminal. Never run `crono login` yourself, because it needs their password. Report any other error as it is.

## Logging food

1. Find the food with `crono search <words>`. Each line is `#<id>  name  [database]  (default measure)`.
2. If the amount isn't in grams or ounces, check the food's measures with `crono food <id>`.
3. Tell the user exactly what you will log and wait for a yes: the food's name and id, the amount, the meal and the day. For several foods, list them all and ask once.
4. Log by id, so crono can't pick a different search result: `crono log '#455994' -a 120g -m dinner -d today`. A search term instead of an id logs the first result.
5. Repeat the `logged:` line to the user. It names the food, the amount, the meal, the date and the estimated calories.

If `log` or `rm` fails, the change may still have reached Cronometer. Before retrying, check for the entry in `crono export <day>`, which fails rather than showing an empty diary when Cronometer's answer is incomplete. If the export fails or you still can't tell, stop and tell the user.

Amounts are `120g`, `5oz`, `1.5 cup`, `3/4 cup`, `"1 1/2 cup"`, `"1 ½ cup"`, or a bare number such as `2` for two of the food's default measure. Quote an amount that contains a space. Without `-a`, crono logs one default measure.

crono matches a unit to the food's measures by exact name, then by a name that starts with it, then by one that contains it. A measure named with a quantity counts as that much: if the only cup measure is "1/2 cup", `-a "1 cup"` logs two of it. Check `crono food <id>` when the portion matters.

Meals are `-m breakfast`, `lunch`, `dinner`, `snacks` or `uncategorized`, or their first letters. Without `-m`, crono picks by the hour in the account's timezone: 04 to 09 breakfast, 10 to 13 lunch, 14 to 20 dinner, and snacks otherwise. Pass `-m` whenever the user named a meal.

Days are `-d today`, `yesterday`, `tomorrow`, `-N` for N days ago, or `YYYY-MM-DD`, in the account's timezone.

## Rules

- Never run `log` or `rm` without the user's yes to that exact entry. A yes covers the entries you listed and nothing more.
- If the top search result isn't clearly the food the user meant, show the top few with their ids and ask. Brand, cooked or raw, and prepared or packaged all change the numbers.
- If crono says "no measure matching", show the measures it lists and ask which one, or use a weight if the user gave one.
- Remove only entries you logged in this conversation, or ones the user points to. Get the id from `crono day`, confirm it, then run `crono rm <id> -d <day>`.
- Don't use `log` or `rm` to test whether crono works. `crono day` and `crono search` are enough.
- The calories crono prints are estimates. `crono day` shows `?` for an entry it can't estimate and leaves it out of the total, which then says how many entries it left out. It shows a recipe entry in servings, not grams. Cronometer's own app is the source of truth.
- `crono export <day>` prints the day's raw diary and each food's full nutrient data as JSON, and never logs in. Check `missing_food_ids` before treating the food data as complete.

## Updating this skill

To update this file after upgrading crono, run the same `crono skill install` command, with the same `--dir` if one was used. It leaves a linked skill folder alone and replaces a file that differs.
