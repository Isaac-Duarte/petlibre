# Writing docs

This page is how we write Wellfed's docs. Most readers own a feeder and want to know what Wellfed does and how far along it is. Some go on to learn how it works, or to help build it.

## One page, one job

Every page does one of three things:

- A **guide** walks you through a task: build the firmware, read the flash, pair the CLI.
- A **reference** holds facts you look up: the pin map, the flash layout, the HTTP routes.
- An **explanation** says how something works and why it was built that way: the feeding owner, OTA banks, pairing.

The first paragraph says what the page covers and who it's for. If a page is doing two jobs, split it.

## Voice

- Talk to the reader as "you", and call the project "we".
- Keep sentences short: about 20 words, one idea each. If a sentence has three commas and a semicolon, it is two sentences.
- Put a caveat next to the thing it qualifies, as an "If…" clause. Avoid chains of parentheses.
- Say when nothing is needed or nothing happens: "No feeder needed." "This does not touch the device."
- Prefer the plain word: "use", not "utilize"; "the feeder", not "the research unit" (except in Evidence sections, where it matters which unit).

## Current facts in the body, evidence at the end

The body states what is true now and explains it. It doesn't tell the story of how we found out.

How we know goes in a short `## Evidence` section at the end of the page. Each entry carries its [evidence level](../evidence.md) (observed, recovered, modeled), the date, and the capture or review names that back it. Keep it to what supports the claims above it.

Board card ids (the `FW…`, `FP…` and `SV…` labels) never appear in tracked docs, and `cargo xtask check-repository` refuses them. Work history lives in the ignored `.agents/` folder and in commit messages. The public summary of what works is the [roadmap](../roadmap.md).

## Status

Status tables use one closed set of words:

| Word | Meaning |
| --- | --- |
| **Works** | Done and observed on a real feeder. |
| **Partial** | Works on a real feeder, with a known gap. |
| **Experimental** | Built and tested on the host or in simulation; not yet confirmed on hardware. |
| **Not started** | Planned, nothing built yet. |

Put one word in each cell, plus a short note at most. Longer caveats go in a Notes list under the table.

## Guides

- Number the steps. Start each step with the command, then say what you should see.
- Before any step that writes flash or can't be undone, add a step that checks you're about to do the right thing.
- End with **Next steps**. Add a **Common mistakes** section when people trip over the same thing.

## Warnings

Use GitHub callouts sparingly:

```markdown
> [!WARNING]
> This erases bank 0. Make a full backup first.
```

Keep `WARNING` for bricking, data loss and electrical or battery hazards. Use `NOTE` for things that are easy to miss. Place a callout at the step it applies to, not at the top of the page.

## Explanations

Lead with the one idea that makes the rest make sense. Then follow one concrete thing through the system, such as a single feed from the button press to the event the app sees. Link out to the detailed pages rather than repeating them.

## Lists of crates and tools

Don't explain a crate list in a paragraph. Use a table:

| I want to… | Use | Read |
| --- | --- | --- |
| Read or write the feeder's flash over UART | `wellfed-flash` | [Rust flash client](rust-flash.md) |

Each full inventory lives in one place: tools in [src/tools/README.md](../../src/tools/README.md), and firmware crates in the [firmware guide](../../src/firmware/wellfed/README.md).

## Device pages

Every device folder starts from [the template](../devices/_template/README.md), so all devices answer the same questions in the same order: what it's sold as, how to identify it, the SoC and flash, UART access, recovery, status and notes.

## Before you finish

- Run `make check-docs`. It checks links, card ids and private paths.
- Read the page once from the top as a newcomer would. If you'd skim a paragraph, cut it.
