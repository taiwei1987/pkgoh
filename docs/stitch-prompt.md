# Stitch Design Prompt for pkgoh

Design a new terminal-first TUI experience for `pkgoh`, a macOS terminal asset manager.

## Product Summary

`pkgoh` scans developer tools installed through multiple ecosystems on macOS and helps users review, filter, evaluate, remove, and clean them from a single interface.

Supported ecosystems:

- Homebrew formulas
- Homebrew casks
- npm global packages
- pnpm global packages
- cargo installed binaries
- pip global packages
- uv-managed Python runtimes
- uv tool installs
- Mac App Store apps via `mas`

The user launches it by typing:

- `pkgoh`
- or `pkg`

## Core Jobs To Be Done

The user needs to:

- understand what is installed
- identify large assets
- identify stale assets
- search for a known tool quickly
- select multiple tools
- see estimated reclaimable space in real time
- understand deletion risk before acting
- delete tools
- clean caches
- avoid destructive mistakes

## Existing Problems To Solve

The current TUI is functional but not polished enough. Problems include:

- weak information hierarchy
- confirmation area not visually strong enough
- state transitions are not expressive enough
- the product does not yet feel like a finished, intentionally designed terminal tool
- some warnings and permission-related interactions are not prominent enough

## Design Goals

Create a terminal-native interface that feels:

- structured
- modern
- calm but confident
- information-dense without becoming chaotic
- clearly action-oriented
- safe for destructive operations

The result should feel like a real asset management console for developers, not just a table and a text box.

## Must-Have Information

### Main list

Each asset row should represent:

- index
- name
- source
- version
- size
- last-used time

### Details panel

The detail area should include:

- name
- source
- version
- size
- last-used time
- tags (large, stale, etc.)
- removal advice tier
- explanation of the advice
- whether cache cleanup is available
- plain-language tool summary
- path / location details

### Summary/header area

The top area should clearly present:

- source scope
- sort state
- selected count
- estimated reclaimable space
- total item count
- search state if active

## Removal Advice Tiers

Every asset belongs to one of three tiers:

- Removable
- Keep Recommended
- Core Dependency

This tiering must be visually clear, especially in the detail panel and action confirmation flow.

## Required States To Design

Please include design treatment for:

- initial loading
- loaded/default state
- empty state
- search state
- multi-select state
- delete confirmation state
- cache cleanup confirmation state
- refresh confirmation state
- quit confirmation state
- permission / password prompt state
- in-progress execution state
- success feedback state
- failure feedback state

## Critical Interaction Requirements

- keyboard-first
- no mouse dependency
- no hover dependency
- no web-style modal assumptions
- no hidden destructive steps
- dangerous actions must be visually obvious
- long-running actions must never look frozen
- permission prompts must feel integrated into the TUI, not external to it

## Localization Requirement

The product supports Simplified Chinese and English.

Please make sure the design can tolerate:

- longer Chinese labels in some contexts
- longer English explanatory text in other contexts
- bilingual layout stability

## Terminal Constraints

This is a real TUI, not a web app.

Please design within these constraints:

- terminal environment only
- character-cell layout mindset
- keyboard interaction only
- no floating graphic windows
- no pixel-perfect assumptions
- should scale down reasonably on narrower terminal widths

## What To Output

Please produce a TUI redesign proposal that includes:

- overall layout concept
- region responsibilities
- visual hierarchy strategy
- state-by-state layout behavior
- confirmation and danger-action treatment
- loading and in-progress feedback treatment
- narrow-width fallback strategy

If possible, provide:

- ASCII layout sketches
- multiple layout alternatives
- recommended final direction with rationale
