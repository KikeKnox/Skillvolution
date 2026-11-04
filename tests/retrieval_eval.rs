//! Retrieval benchmark: a fixed, generic vault (`tests/fixtures/retrieval/skills.json`)
//! queried with realistic Spanish and English prompts plus agent-style keyword queries
//! (`tests/fixtures/retrieval/queries.json`). It measures explicit search
//! (`Vault::search`) and unprompted prompt-time suggestions (`hook::relevant_skills`),
//! prints a metrics table and every failing query (`cargo test --test retrieval_eval
//! -- --nocapture`), and asserts regression floors and ceilings.
//!
//! The fixtures were written before any measurement and must not be tuned to the
//! results: change retrieval, not the benchmark. Add queries freely (and re-baseline),
//! but never edit one just because it fails.

mod support;

use serde::Deserialize;
use skillvolution::{hook, vault::Vault};
use std::collections::{BTreeMap, HashSet};
use support::Draft;

const SEARCH_LIMIT: i64 = 10;

#[derive(Deserialize)]
struct Skill {
    id: String,
    description: String,
    tags: Vec<String>,
    #[serde(default)]
    deprecated: bool,
    when_to_use: String,
    procedure: Vec<String>,
    pitfalls: Vec<String>,
    verification: String,
}

impl Skill {
    fn content(&self) -> String {
        let procedure: Vec<String> = self
            .procedure
            .iter()
            .enumerate()
            .map(|(i, step)| format!("{}. {step}", i + 1))
            .collect();
        let pitfalls: Vec<String> = self.pitfalls.iter().map(|p| format!("- {p}")).collect();
        format!(
            "## When to use\n{}\n\n## Procedure\n{}\n\n## Pitfalls\n{}\n\n## Verification\n{}\n",
            self.when_to_use,
            procedure.join("\n"),
            pitfalls.join("\n"),
            self.verification
        )
    }
}

#[derive(Deserialize)]
struct Query {
    text: String,
    lang: String,
    kind: String,
    expected: Option<String>,
}

fn load<T: for<'de> Deserialize<'de>>(name: &str) -> T {
    let path = format!(
        "{}/tests/fixtures/retrieval/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Checks the fixtures are well-formed, so a broken edit fails loudly instead of
/// silently shifting the numbers.
fn check_fixtures(skills: &[Skill], queries: &[Query]) {
    let mut ids = HashSet::new();
    for skill in skills {
        assert!(ids.insert(skill.id.as_str()), "duplicate id {}", skill.id);
        assert!(
            skill.description.starts_with("Use when ") && skill.description.len() <= 280,
            "{}: description must start with \"Use when \" and fit in 280 bytes",
            skill.id
        );
        assert!(
            (2..=6).contains(&skill.tags.len()),
            "{}: 2-6 tags",
            skill.id
        );
    }
    let live: Vec<&Skill> = skills.iter().filter(|s| !s.deprecated).collect();
    for query in queries {
        assert!(
            matches!(query.lang.as_str(), "es" | "en"),
            "{}: lang",
            query.text
        );
        assert!(
            matches!(query.kind.as_str(), "prompt" | "keywords"),
            "{}: kind",
            query.text
        );
        if let Some(expected) = &query.expected {
            assert!(
                live.iter().any(|s| &s.id == expected),
                "{}: expected {expected} is not a live skill",
                query.text
            );
        } else {
            assert_eq!(
                query.kind, "prompt",
                "{}: negatives are prompts",
                query.text
            );
        }
    }
    for skill in live {
        let count = |lang: &str, kind: &str| {
            queries
                .iter()
                .filter(|q| {
                    q.expected.as_deref() == Some(skill.id.as_str())
                        && q.lang == lang
                        && q.kind == kind
                })
                .count()
        };
        assert!(
            count("es", "prompt") >= 2
                && count("en", "prompt") >= 2
                && count("en", "keywords") >= 1,
            "{}: needs 2 ES prompts, 2 EN prompts, and 1 keyword query",
            skill.id
        );
    }
}

fn build_vault(dir: &std::path::Path, skills: &[Skill]) -> Vault {
    let mut vault = Vault::open(&dir.join("skills.db")).unwrap();
    for skill in skills {
        let tags: Vec<&str> = skill.tags.iter().map(String::as_str).collect();
        Draft::new(&skill.id)
            .description(&skill.description)
            .tags(&tags)
            .raw_content(&skill.content())
            .publish(&mut vault);
        if skill.deprecated {
            vault.set_deprecated(&skill.id, true).unwrap();
        }
    }
    vault
}

#[derive(Default)]
struct SearchStats {
    n: usize,
    hit1: usize,
    hit3: usize,
    reciprocal_rank: f64,
}

impl SearchStats {
    fn recall1(&self) -> f64 {
        ratio(self.hit1, self.n)
    }
    fn recall3(&self) -> f64 {
        ratio(self.hit3, self.n)
    }
    fn mrr(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.reciprocal_rank / self.n as f64
        }
    }
}

#[derive(Default)]
struct HookStats {
    positives: usize,
    correct: usize,
    first: usize,
    exact: usize,
    wrong: usize,
    extra: usize,
    negatives: usize,
    false_positives: usize,
}

impl HookStats {
    /// Positive prompts whose expected skill is among the suggestions.
    fn correct_rate(&self) -> f64 {
        ratio(self.correct, self.positives)
    }
    /// Positive prompts whose first suggestion is the expected skill.
    fn first_rate(&self) -> f64 {
        ratio(self.first, self.positives)
    }
    /// Positive prompts whose only suggestion is the expected skill.
    fn exact_rate(&self) -> f64 {
        ratio(self.exact, self.positives)
    }
    /// Positive prompts with suggestions that all miss the expected skill.
    fn wrong_rate(&self) -> f64 {
        ratio(self.wrong, self.positives)
    }
    /// Suggestions other than the expected skill, per positive prompt.
    fn extra_per_prompt(&self) -> f64 {
        if self.positives == 0 {
            0.0
        } else {
            self.extra as f64 / self.positives as f64
        }
    }
    /// Negative prompts that got any suggestion.
    fn false_positive_rate(&self) -> f64 {
        ratio(self.false_positives, self.negatives)
    }
}

fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// Allowed slack around each baseline before the benchmark calls it a regression.
const TOLERANCE: f64 = 0.05;
/// Extra suggestions per prompt is a count, not a rate, so it gets more slack.
const EXTRA_TOLERANCE: f64 = 0.10;

/// Whether a larger value of a metric is better (recall) or worse (false positives).
#[derive(Clone, Copy)]
enum Better {
    Higher,
    Lower,
}

/// Records a violation when `value` is worse than `baseline` by more than `tolerance`.
fn check(
    violations: &mut Vec<String>,
    name: &str,
    value: f64,
    baseline: f64,
    better: Better,
    tolerance: f64,
) {
    // Rounded so float noise at exactly the bound never fails.
    let value = (value * 10_000.0).round() / 10_000.0;
    let regressed = match better {
        Better::Higher => value < baseline - tolerance,
        Better::Lower => value > baseline + tolerance,
    };
    if regressed {
        violations.push(format!(
            "{name} = {value:.3} regressed from the baseline {baseline:.3} (tolerance {tolerance:.2})"
        ));
    }
}

#[test]
fn retrieval_benchmark() {
    let skills: Vec<Skill> = load("skills.json");
    let queries: Vec<Query> = load("queries.json");
    check_fixtures(&skills, &queries);
    let deprecated: HashSet<&str> = skills
        .iter()
        .filter(|s| s.deprecated)
        .map(|s| s.id.as_str())
        .collect();

    let dir = tempfile::tempdir().unwrap();
    let vault = build_vault(dir.path(), &skills);

    let mut search: BTreeMap<String, SearchStats> = BTreeMap::new();
    let mut hook_stats: BTreeMap<String, HookStats> = BTreeMap::new();
    let mut search_failures = Vec::new();
    let mut hook_failures = Vec::new();

    for query in &queries {
        let found: Vec<String> = vault
            .search(&query.text, None, SEARCH_LIMIT, 0)
            .unwrap()
            .skills
            .into_iter()
            .map(|s| s.id)
            .collect();
        // A deprecated skill must never surface, whatever the query.
        for id in &found {
            assert!(
                !deprecated.contains(id.as_str()),
                "deprecated {id} returned for {:?}",
                query.text
            );
        }

        if let Some(expected) = &query.expected {
            let group = format!("{}/{}", query.lang, query.kind);
            let rank = found.iter().position(|id| id == expected).map(|i| i + 1);
            for key in [group.as_str(), "all"] {
                let stats = search.entry(key.to_owned()).or_default();
                stats.n += 1;
                if let Some(rank) = rank {
                    stats.hit1 += usize::from(rank == 1);
                    stats.hit3 += usize::from(rank <= 3);
                    stats.reciprocal_rank += 1.0 / rank as f64;
                }
            }
            if rank.is_none_or(|rank| rank > 1) {
                let rank = rank.map_or("-".to_owned(), |r| r.to_string());
                let top: Vec<&str> = found.iter().take(3).map(String::as_str).collect();
                search_failures.push(format!(
                    "  [{}/{}] rank {rank:>2} want {expected}: {:?}  top3={top:?}",
                    query.lang, query.kind, query.text
                ));
            }
        }

        if query.kind != "prompt" {
            continue;
        }
        let suggested: Vec<String> = hook::relevant_skills(&vault, None, &query.text)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        for key in [query.lang.as_str(), "all"] {
            let stats = hook_stats.entry(key.to_owned()).or_default();
            match &query.expected {
                Some(expected) => {
                    stats.positives += 1;
                    let hit = suggested.contains(expected);
                    stats.correct += usize::from(hit);
                    stats.first += usize::from(suggested.first() == Some(expected));
                    stats.exact += usize::from(suggested == [expected.as_str()]);
                    stats.wrong += usize::from(!hit && !suggested.is_empty());
                    stats.extra += suggested.iter().filter(|id| *id != expected).count();
                }
                None => {
                    stats.negatives += 1;
                    stats.false_positives += usize::from(!suggested.is_empty());
                }
            }
        }
        let expected = query.expected.as_deref();
        let clean = match expected {
            Some(expected) => suggested == [expected],
            None => suggested.is_empty(),
        };
        if !clean {
            hook_failures.push(format!(
                "  [{}] want {}: {:?}  got={suggested:?}",
                query.lang,
                expected.unwrap_or("nothing"),
                query.text
            ));
        }
    }

    println!(
        "\n== Retrieval benchmark: {} skills ({} deprecated), {} queries ==",
        skills.len(),
        deprecated.len(),
        queries.len()
    );
    println!("\nSearch (Vault::search, limit {SEARCH_LIMIT}, positive queries)");
    println!(
        "  {:<12} {:>4} {:>9} {:>9} {:>7}",
        "group", "n", "recall@1", "recall@3", "MRR"
    );
    for (group, s) in &search {
        println!(
            "  {group:<12} {:>4} {:>9.3} {:>9.3} {:>7.3}",
            s.n,
            s.recall1(),
            s.recall3(),
            s.mrr()
        );
    }
    println!("\nPrompt hook (hook::relevant_skills, prompts only)");
    println!(
        "  {:<6} {:>4} {:>8} {:>7} {:>7} {:>7} {:>7} {:>4} {:>7}",
        "lang", "pos", "correct", "first", "exact", "wrong", "extra", "neg", "FP"
    );
    for (lang, h) in &hook_stats {
        println!(
            "  {lang:<6} {:>4} {:>8.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>4} {:>7.3}",
            h.positives,
            h.correct_rate(),
            h.first_rate(),
            h.exact_rate(),
            h.wrong_rate(),
            h.extra_per_prompt(),
            h.negatives,
            h.false_positive_rate()
        );
    }
    println!("\nSearch misses (expected skill not ranked first):");
    for line in &search_failures {
        println!("{line}");
    }
    println!(
        "\nHook misses (suggestions differ from exactly the expected skill, or anything for a negative):"
    );
    for line in &hook_failures {
        println!("{line}");
    }

    // Regression baselines. History: the benchmark was introduced on keywords +
    // the id/tag/description gate, measured at hook FP 0.719, extra 1.01, correct
    // 0.98, exact 0.31. The precision change (2-keyword gate + relative score
    // cutoff + ES->EN keyword map + porter stemming) moved the hook to FP 0.125,
    // extra 0.14, correct 0.91, exact 0.78; the recall drop (correct 0.98 -> 0.91)
    // was accepted on purpose in exchange for far fewer false positives. A metric
    // fails only when it is worse than its baseline by more than TOLERANCE
    // (EXTRA_TOLERANCE for extra suggestions), so the floors sit 0.05 below and the
    // ceilings 0.05 above these numbers. The false-positive baselines are capped at
    // 0.15 (es measures 0.188) so the ceiling never exceeds 0.20 and a regression
    // towards the old 0.72 is caught. Tighten when retrieval improves; loosen one
    // only with a written reason in the commit that does it.
    let mut violations = Vec::new();
    // group, recall@1, recall@3, MRR
    for (group, recall1, recall3, mrr) in [
        ("es/prompt", 0.880, 1.000, 0.940),
        ("en/prompt", 0.960, 1.000, 0.977),
        ("en/keywords", 1.000, 1.000, 1.000),
        ("all", 0.937, 1.000, 0.967),
    ] {
        let s = &search[group];
        for (metric, value, baseline) in [
            ("recall@1", s.recall1(), recall1),
            ("recall@3", s.recall3(), recall3),
            ("MRR", s.mrr(), mrr),
        ] {
            let name = format!("search {group} {metric}");
            check(
                &mut violations,
                &name,
                value,
                baseline,
                Better::Higher,
                TOLERANCE,
            );
        }
    }
    // lang, correct, first, exact, wrong, extra per prompt, false positives
    for (lang, correct, first, exact, wrong, extra, false_positive) in [
        ("es", 0.920, 0.920, 0.760, 0.000, 0.180, 0.15),
        ("en", 0.900, 0.900, 0.800, 0.000, 0.100, 0.0625),
        ("all", 0.910, 0.910, 0.780, 0.000, 0.140, 0.125),
    ] {
        let h = &hook_stats[lang];
        for (metric, value, baseline, better, tolerance) in [
            (
                "correct",
                h.correct_rate(),
                correct,
                Better::Higher,
                TOLERANCE,
            ),
            ("first", h.first_rate(), first, Better::Higher, TOLERANCE),
            ("exact", h.exact_rate(), exact, Better::Higher, TOLERANCE),
            ("wrong", h.wrong_rate(), wrong, Better::Lower, TOLERANCE),
            (
                "extra/prompt",
                h.extra_per_prompt(),
                extra,
                Better::Lower,
                EXTRA_TOLERANCE,
            ),
            (
                "false positives",
                h.false_positive_rate(),
                false_positive,
                Better::Lower,
                TOLERANCE,
            ),
        ] {
            let name = format!("hook {lang} {metric}");
            check(&mut violations, &name, value, baseline, better, tolerance);
        }
    }
    assert!(
        violations.is_empty(),
        "retrieval regressed:\n{}",
        violations.join("\n")
    );
}
