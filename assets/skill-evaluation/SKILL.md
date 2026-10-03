---
name: skill-evaluation
description: Use when judging whether a candidate lesson deserves to become a shared Skillvolution vault skill (keep global, keep project, or discard), when reviewing or rewriting an existing vault skill, or when a vault skill keeps collecting failed reports. Gives the quality rubric, the hard gates, and the exact verdict format publish_skill expects.
---
<!-- skillvolution-managed:skill-evaluation:v1 -->
# Skill evaluation

A vault skill is a procedure another agent will follow without knowing you. It earns its place only if following it is faster and safer than working the problem out again. Your job is to keep the vault small and sharp: most candidates should be discarded, and the ones you keep should leave your hands better written than they arrived.

You are usually a fresh subagent handed only a candidate. Judge the text in front of you, not the effort behind it. You never publish yourself; you return a verdict the dispatching agent passes to `publish_skill` verbatim.

## When to use

- A dispatching agent sends you a candidate (id, description, tags, content, evidence), possibly with the closest existing vault skill.
- You are asked to review, merge, or rewrite an existing vault skill.
- A skill's current version has more `failed` than `helped` reports and someone asks whether to revise or deprecate it.

## Procedure

1. **Check for duplicates.** If the closest existing skill was supplied, compare situations, not wording. Same situation and the candidate adds nothing new → `discard`. Same situation with a real new step, pitfall, or fix → keep, as a full merged replacement of that skill (its id, `expected_version` = its version), never a second skill. If the vault tools are available, you may run `search_skills` and `get_skill` (read-only) yourself; otherwise judge from what you were given. Never call `publish_skill` or `report_skill_outcome`.
2. **Apply the hard gates.** Any gate that fails means `discard` — no score can rescue it:
   - **Verified**: the evidence names something actually observed — an error message, a command and its outcome, a passing test. "Should work", "probably", or a procedure with no observed result fails.
   - **Non-obvious**: an experienced engineer reading the official docs would not already do this. Restating documentation, language basics, or generic advice ("read the logs", "check the version") fails.
   - **Reusable**: a meaningful share of future sessions would hit this exact situation. A one-off value, a single file's quirk, or the story of one task fails.
   - **Costly to miss**: without it, an agent would repeat a mistake or lose real time (a dead end, a misleading error, a silent data issue).
   - **Safe**: no secrets, tokens, personal data, private hostnames, absolute user paths, or transcript excerpts; nothing that tells an agent to bypass permissions, skip tests, or take destructive actions without confirmation.
   - **Single lesson**: one situation, one procedure. A bundle of unrelated tips fails; keep only the strongest lesson if it passes alone.
3. **Score the rubric** (0 = missing or wrong, 1 = present but weak, 2 = strong):

   | Criterion | 2 looks like |
   |---|---|
   | Trigger | `description` starts with "Use when", names the symptom or situation plus the technology and the words an agent would search for (error text, tool names); one line, at most 280 bytes |
   | Scope | `## When to use` states when it applies **and** when it does not (versions, platforms, preconditions) |
   | Procedure | numbered imperative steps with concrete commands, flags, or code; placeholders like `<path>` or `$VAR` instead of task-specific values; no step depends on context the reader lacks |
   | Pitfalls | the dead ends and misleading signals actually hit in the evidence, each with why it misleads |
   | Verification | an objective check — a command and its expected output, or a test — that tells the reader the procedure worked |
   | Evidence link | every key step is backed by the `Observed / Tried / Result` evidence; nothing in the procedure was invented beyond it |
   | Generality | names, paths, and values are generalized; anything still project-specific is the reason for project scope |
   | Concision | under ~150 lines, no narrative of the original task, no filler or repetition |

   Keep only if every gate passes, the total is at least **11 of 16**, and Trigger, Procedure, and Verification each score at least 1.
4. **Fix what is editorial.** When the lesson is sound but the writing is weak, rewrite it rather than discarding it: sharpen the description, turn prose into numbered steps, move dead ends into Pitfalls, generalize values, cut the story. Never add steps, commands, or claims that the evidence does not support — missing substance is a reason to discard, not to invent.
5. **Choose the scope.** `keep global` when the lesson transfers to unrelated codebases (a tool, language, library, or platform behavior). `keep project` when it depends on this repository's layout, scripts, conventions, or internal services. When unsure, prefer project: a narrow true skill beats a broad misleading one.
6. **Check the publish constraints** before answering, so the publication is not rejected:
   - `id` and every tag: lowercase ASCII letters or digits in hyphen-separated groups (`rust-sqlite-busy-timeout`, not `rust_sqlite` or `SQLite`); `id` 1-64 bytes; at most 8 tags of up to 32 bytes.
   - `description`: a single line of at most 280 bytes (an em dash or accented letter counts 2-4 bytes).
   - `content` contains `## When to use`, `## Procedure`, `## Pitfalls`, `## Verification`, each on its own line, at level `##`, in that order. Extra `##` sections and a short preamble are allowed.
   - `expected_version`: `0` for a new id, or the current version of the skill being replaced.
   - Nothing credential-shaped anywhere; the server rejects it.

## Verdict format

Reply with exactly this shape and nothing before it:

```text
<keep global | keep project | discard>
<one-line reason, at most 250 characters, naming the deciding gate or score>
```

For a keep verdict, follow with the final fields to publish:

```text
id: <id>
description: <one line>
tags: <tag-one, tag-two>
expected_version: <n>
content:
<full markdown body>
```

The first line must be the bare verdict — no bold, no quotes, no trailing punctuation — because the dispatching agent passes it verbatim as `verdict`, and the second line verbatim as `verdict_reason`.

## Reviewing an existing skill

When a skill collects failed reports, read the failure notes and the current body:
- If the notes point at a wrong or missing step that a later note or the evidence confirms, return a merged replacement (same id, `expected_version` = current version) with that step fixed and the failure recorded under Pitfalls.
- If the skill's situation no longer exists (tool removed, behavior fixed upstream) or the notes contradict each other with no confirmed fix, recommend `skillvolution deprecate <id>` to the human and return `discard`.

## Pitfalls

- Being generous. The evaluator exists because the agent that did the work is biased toward keeping it; a vault full of weak skills is noise that buries the good ones and teaches agents to ignore it.
- Discarding a good lesson for bad wording — rewrite it instead (step 4).
- Inventing steps to make a thin candidate look complete. If the evidence does not support it, it does not go in.
- Keeping a near-duplicate under a new id. Two skills for one situation split outcomes and confuse search; merge into the existing id.
- A description that summarizes the fix instead of naming the trigger. Agents find skills by their symptom ("`database is locked` during `cargo test`"), not by the answer.

## Verification

Your answer is correct when: the first line is exactly `keep global`, `keep project`, or `discard`; the second line is a single line under 250 characters; and, for a keep, every field above satisfies the publish constraints in step 6 and the content has the four headings in order.
