# crono

Log food to your [Cronometer](https://cronometer.com) diary from the terminal.

```console
$ crono log '#458689' -a "3/4 cup" -m breakfast
logged: Fage, Total 2% Greek Yogurt, Plain — 0.75 × cup → breakfast on 2026-10-14 ≈ 130 kcal
```

## Install

You need [Rust](https://rustup.rs).

```sh
cargo install --locked --git https://github.com/adityapidaparti/crono
crono login you@example.com --remember
```

### With an AI agent

Give your agent this repository's link and ask it to install crono. It should:

1. Run `cargo install --locked --git https://github.com/adityapidaparti/crono`.
2. Run `crono skill install`, which adds the crono skill for Claude Code and
   Grok. For Codex, run `crono skill install --dir ~/.agents/skills`.
3. Ask you to run `crono login you@example.com --remember` yourself, since it
   asks for your password.

## Use

```sh
crono search hummus                         # find a food and its id
crono food 455994                           # its measures and macros
crono log hummus -a 120g -m dinner          # log the top search result
crono log '#455994' -a "2 tbsp" -d yesterday
crono day                                   # today's diary, with calories
crono rm 5300000001                         # remove an entry, by the id `crono day` shows
crono export yesterday > day.json           # a day's raw diary and nutrient data
```

An amount can be `120g`, `5oz`, `3/4 cup`, `1 1/2 cup`, `1 ½ cup`, or a bare
number of the food's default measure. Without `-m`, crono picks the meal from the time of
day. `crono --help` covers the rest.

## Good to know

- Cronometer has no public API. crono uses the one Cronometer's Android app
  uses, which can change without notice.
- `--remember` saves your password, unencrypted, in a file only your user can
  read, so crono can log in again when the session expires.

## Tests

Tested with [Hegel](https://hegel.dev) property-based tests.
