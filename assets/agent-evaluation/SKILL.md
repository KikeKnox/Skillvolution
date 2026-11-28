---
name: agent-evaluation
description: Use when judging whether a candidate subagent deserves to become a shared Skillvolution vault agent (keep global, keep project, or discard), when reviewing or rewriting an existing vault agent, or when a vault agent keeps collecting failed reports. Gives the agent rubric, the hard gates, and the exact verdict format publish_agent expects.
---
<!-- skillvolution-managed:agent-evaluation:v1 -->
# Agent evaluation

A vault agent is a subagent another orchestrator will delegate to without knowing you: it gets a task, runs its own prompt, and hands back what its output contract promises. It earns its place only if delegating to it is cheaper and more reliable than writing a task prompt for a generic worker. Your job is to keep the vault small and sharp: most candidates should be discarded, and the ones you keep should leave your hands better written than they arrived.

You are usually a fresh subagent handed only a candidate. Judge the text in front of you, not the effort behind it. You never publish yourself; you return a verdict the dispatching agent passes to `publish_agent` verbatim.

## When to use

- A dispatching agent sends you a candidate agent (id, description, tier, effort, tools, skills, content, evidence), possibly with the id of the closest existing vault agent.
- You are asked to review, merge, or rewrite an existing vault agent.
- An agent's current version has more `failed` than `helped` reports and someone asks whether to revise or deprecate it.
- Not for lessons or procedures: those are skills, judged with the `skill-evaluation` skill.

## Procedure

1. **Check for duplicates.** If the vault tools are available, run `search_agents` (read-only) with the candidate's work as keywords; if you have a shell, read the closest agent's prompt with `skillvolution get <id>`. Same work and the candidate adds nothing → `discard`. Same work with a real improvement → keep, as a full merged replacement of that agent (its id, `expected_version` = its version), never a second agent. A candidate that only restates what the built-in workers `sv-high`, `sv-mid`, `sv-low` do with a good task prompt → `discard`. Never call `install_agent`, `publish_agent`, `publish_skill` or `report_skill_outcome`.
2. **Apply the hard gates.** Any gate that fails means `discard` — no score can rescue it:
   - **Bounded role**: one kind of work, named in the description and `## Role`, with what is out of scope. A "does anything" or "senior engineer" agent fails.
   - **No delegation**: nothing in the prompt tells the agent to spawn, call, or hand work to another agent or subagent (`Agent`, `Task`, `task`, `subagent`, `run_subagent`, "ask another agent"); `tools` is declared, because leaving it out grants every tool and leaves delegation allowed in the generated file.
   - **Verifiable output contract**: `## Output contract` fixes what comes back — named fields in a set order, a result status (done, partial, blocked), and the checks the agent ran — so the orchestrator can verify it without redoing the work. "A summary of what you did" fails.
   - **Listed skills exist**: every id in `skills` is a published skill visible from the agent's scope (check with `search_skills` when available), and `tools` includes `mcp` to load them. A project-scoped skill means the agent must be `keep project`.
   - **Verified**: the evidence names at least one real delegation and what it returned. A prompt that was never run fails.
   - **Safe**: no secrets, tokens, personal data, private hostnames, absolute user paths, or transcript excerpts; nothing that tells the agent to bypass permissions, skip tests, commit, push, or delete without the task saying so.
3. **Score the rubric** (0 = missing or wrong, 1 = present but weak, 2 = strong):

   | Criterion | 2 looks like |
   |---|---|
   | Trigger | `description` starts with "Use when", names the work an orchestrator would delegate and the words it would search for; one line, at most 280 bytes |
   | Role | `## Role` states the one job and what the agent leaves alone |
   | Tier | fits the work: `low` for search, summaries, mechanical edits; `mid` for bounded implementation, tests, review; `high` for design, hard debugging, ambiguous problems. `effort` only when the tier's default does not fit |
   | Tools | minimal: only what the procedure uses (`read` is always granted); `edit` only if it changes files, `shell` only if it runs commands, `web` and `mcp` only with a reason in the prompt |
   | Procedure | numbered imperative steps the agent can follow with only the task it is given; no step depends on the orchestrator's context |
   | Output contract | short, ordered, checkable fields as in the gate, with "none" stated for empty ones |
   | Evidence link | the role, tools, and contract match what the evidenced delegations needed and returned; nothing invented beyond them |
   | Concision | under ~80 lines, no narrative of the original task, no filler or repetition |

   Keep only if every gate passes, the total is at least **11 of 16**, and Role, Tier, and Output contract each score at least 1.
4. **Fix what is editorial.** When the agent is sound but the writing is weak, rewrite it rather than discarding it: sharpen the description, cut tools the procedure never uses, lower a tier the work does not need, turn prose into numbered steps, tighten the output contract, add a `## Limits` line for what it must not do. Never add capabilities, steps, or claims that the evidence does not support — missing substance is a reason to discard, not to invent.
5. **Choose the scope.** `keep global` when the role transfers to unrelated codebases. `keep project` when it depends on this repository's layout, scripts, conventions, or internal services, or lists a project-scoped skill. When unsure, prefer project.
6. **Check the publish constraints** before answering, so the publication is not rejected:
   - `id` and every tag: lowercase ASCII letters or digits in hyphen-separated groups; `id` 1-64 bytes, not starting with `sv-`, and not a client built-in agent or managed skill name (`general-purpose`, `explore`, `plan`, `build`, `general`, `evolution`, ...); at most 8 tags of up to 32 bytes.
   - `description`: a single line of at most 280 bytes (an em dash or accented letter counts 2-4 bytes).
   - `tier`: `high`, `mid`, or `low`. `effort`, if set: `low`, `medium`, `high`, `xhigh`, or `max`. `tools`: required and non-empty, any of `read`, `edit`, `shell`, `web`, `mcp`. `skills`: at most 8 skill ids.
   - `content` contains `## Output contract`; `## Role`, `## Procedure`, and `## Limits` are optional, and the ones present appear in the order Role, Procedure, Output contract, Limits.
   - `expected_version`: `0` for a new id, or the current version of the agent being replaced.
   - Nothing credential-shaped anywhere; the server rejects it.

## Verdict format

Reply with exactly this shape and nothing before it:

```text
<keep global | keep project | discard>
<one-line reason, at most 250 characters, naming the deciding gate or score>
```

For a keep verdict, follow with the final fields to publish (leave out `effort` and `skills` when unset):

```text
id: <id>
description: <one line>
tags: <tag-one, tag-two>
tier: <high | mid | low>
effort: <effort>
tools: <read, shell>
skills: <skill-one, skill-two>
expected_version: <n>
content:
<full markdown prompt>
```

The first line must be the bare verdict — no bold, no quotes, no trailing punctuation — because the dispatching agent passes it verbatim as `verdict`, and the second line verbatim as `verdict_reason`.

## Reviewing an existing agent

When an agent collects failed reports, read the failure notes and the current prompt:
- If the notes point at a wrong step, a missing limit, a contract field the orchestrator could not use, or a tier too low for the work, and a later note or the evidence confirms the fix, return a merged replacement (same id, `expected_version` = current version) with that fixed.
- If the agent's work no longer exists or the notes contradict each other with no confirmed fix, recommend `skillvolution deprecate <id>` to the human and return `discard`.

## Pitfalls

- Being generous. A vault full of overlapping agents makes the orchestrator's choice worse, not better; a good task prompt to `sv-mid` beats a weak specialist.
- Granting tools "just in case". Every extra tool widens what a delegated agent can break, and an omitted `tools` list also lets it delegate in turn.
- A high tier by default. It costs more on every delegation; pick the lowest tier that the evidenced work needed.
- An output contract the orchestrator cannot check ("report back", "explain your changes"): it forces the orchestrator to redo the work to trust it.
- Discarding a sound agent for bad wording — rewrite it instead (step 4); inventing steps or capabilities to make a thin candidate look complete.

## Verification

Your answer is correct when: the first line is exactly `keep global`, `keep project`, or `discard`; the second line is a single line under 250 characters; and, for a keep, every field above satisfies the publish constraints in step 6, `tools` is declared, and the content has `## Output contract` with the other agent headings in order.
