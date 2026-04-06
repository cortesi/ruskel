# Deslop Skill Rewrite Plan

This plan covers a rewrite of `~/.agents/skills/deslop` to make it more useful to coding agents.
The main goals are to reduce context cost, sharpen the workflow, improve trigger coverage, and
preserve the strongest Rust-specific review guidance.

1. Stage One: Define Target Shape

Set the scope and success criteria for the rewrite before changing the skill structure.

1. [x] Confirm the skill's primary job is review-first guidance for Rust code quality, design, and
       refactor planning, not a general Rust style guide.
2. [x] Define the target structure for the skill package, including a lean `SKILL.md`, a
       `references/` directory for detailed review principles, and refreshed `agents/openai.yaml`
       metadata.
3. [x] Decide the core review lenses that stay in `SKILL.md`, collapsing overlapping principles
       into a smaller set such as API boundaries, ownership and allocation, panic and error
       semantics, modularity and locality, parsing and types, and unsafe or concurrency risks.
4. [x] Define review modes for file/module, crate, workspace, and changed-code scopes so the skill
       adapts its depth to the target size.

2. Stage Two: Rewrite The Core Skill

Replace the current manifesto-style body with a short operational playbook that agents can apply
quickly.

1. [x] Rewrite the frontmatter description in `~/.agents/skills/deslop/SKILL.md` so it triggers on
       broader requests such as code review, API audit, simplification, design debt review, and
       refactor planning for Rust code.
2. [x] Replace the current top-level workflow with a deterministic review loop that tells the agent
       what to inspect first, how to triage risk, when to stop reading, and how many findings to
       return by default.
3. [x] Rewrite the output contract so findings emphasize severity, exact location, impact, and
       concrete remediation, with code snippets used only when they materially clarify the issue.
4. [x] Add explicit behavior for review-only requests, review-then-implement requests, mixed-language
       repos, and cases where the target is not meaningfully Rust.

3. Stage Three: Split Reference Material

Move detailed doctrine out of the main skill body so agents only load it when needed.

1. [x] Create `~/.agents/skills/deslop/references/` and move the long principle catalog from
       `SKILL.md` into a small number of focused reference files linked directly from the main skill.
2. [x] Keep only the minimum navigation and selection guidance in `SKILL.md`, including when to read
       each reference file and what questions each one helps answer.
3. [x] Consolidate overlapping principle sections so the reference set is shorter, less repetitive,
       and easier for an agent to consult selectively.
4. [x] Add a compact anti-pattern index in the main skill or a single reference file so common
       signals like clone storms, one-impl traits, panic paths, and pub-surface leakage remain fast
       to detect.

4. Stage Four: Refresh Examples And Metadata

Make the skill easier to trigger and easier for agents to imitate correctly.

1. [x] Replace the single panic-focused worked example with a small example set covering at least
       API leakage, clone pressure, speculative abstraction, and unsafe invariants.
2. [x] Update `~/.agents/skills/deslop/agents/openai.yaml` so the display text and default prompt
       match the rewritten skill and advertise the broader review use cases.
3. [x] Review the rewritten files for consistency between frontmatter, body instructions, examples,
       and agent metadata so the skill does not advertise behaviors it no longer supports.

5. Stage Five: Validate The Rewrite

Check that the new skill is smaller, clearer, and produces better agent behavior on realistic tasks.

1. [x] Verify the new `SKILL.md` stays comfortably below the current size and remains readable after
       reintegrating the detailed guidance inline.
2. [ ] Run a small prompt suite against the rewritten skill using realistic requests such as Rust
       code review, API audit, and simplification prompts, and compare the outputs against the
       current skill for specificity and brevity.
3. [x] Capture any missed trigger phrases or workflow ambiguities discovered during validation and
       fold them back into the plan before finalizing the rewrite.
4. [x] Reintegrate the `references/` content into `SKILL.md` after observed agent behavior shows
       the main skill is loaded reliably but the reference files are not.
