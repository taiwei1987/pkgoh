# pkgoh Product Requirements Document (PRD)

Version: v1
Date: 2026-04-28
Project: pkgoh
Category: macOS terminal asset manager (TUI)

## 1. Product Overview

`pkgoh` is a terminal asset manager for macOS developer environments.

Developers often accumulate globally installed tools, runtimes, CLIs, and desktop apps from multiple package ecosystems, including:

- Homebrew
- npm
- pnpm
- cargo
- pip
- uv
- mas

Over time, users lose visibility into what is installed, what is large, what is stale, and what can be removed safely.

`pkgoh` solves that by providing a keyboard-first, high-density TUI for scanning, reviewing, filtering, evaluating, deleting, and cleaning cached assets from those ecosystems.

## 2. Product Goals

### 2.1 Core Goal

After installation, the user should be able to type:

- `pkgoh`
- or the short alias `pkg`

and immediately enter a unified terminal interface that helps them:

- scan installed developer tools
- understand asset distribution by size, usage, and source
- locate tools quickly
- multi-select targets
- delete unused tools
- clean caches
- understand risk and reclaim value before acting

### 2.2 Product Principles

- scanning must be real, not mocked
- actions must be real, not fake removals
- the UI must always provide feedback and never feel frozen
- the product must reduce the chance of destructive mistakes
- the product must remain understandable for non-expert users
- the product must still feel fast and efficient for terminal users

## 3. Target Users

### 3.1 Primary Users

- developers using macOS
- users who install many CLIs, runtimes, and ecosystem tools
- terminal users who are not necessarily system-maintenance experts

### 3.2 Typical Characteristics

- comfortable installing tools from the command line
- likely to use multiple package managers at once
- unsure what can be safely removed
- prefer a guided interface over memorizing many cleanup commands

## 4. Core Use Cases

### Use Case 1: Inventory

The user wants to understand what is globally installed across supported ecosystems.

### Use Case 2: Find Large Assets

The user wants to recover disk space by identifying the biggest installed tools.

### Use Case 3: Find Stale Tools

The user wants to clean up tools that have not been used for a long time.

### Use Case 4: Safe Deletion

The user wants risk guidance before removing something.

### Use Case 5: Cache Cleanup

The user wants to reclaim space without uninstalling the tool itself.

### Use Case 6: Search and Locate

The user knows a tool name and wants to find it quickly, regardless of source.

## 5. Supported Asset Sources

The current version must support:

- Homebrew formulas
- Homebrew casks
- npm global packages
- pnpm global packages
- cargo installed binaries
- pip global packages
- uv-managed Python runtimes
- uv tool installs
- Mac App Store apps via `mas`

Notes:

- scanning should not be limited to one single canonical directory only
- path-exposed Node tools installed in nonstandard global roots should still be included when possible
- the goal is broad coverage inside supported ecosystems, not blind full-disk scanning

## 6. Required Information Display

Each asset row must show at least:

- index
- name
- source
- version
- size
- last-used time

The detail panel must show at least:

- name
- source
- version
- size
- last-used time
- tags (for example large or stale)
- removal advice tier
- advice explanation
- whether cache cleanup is available
- tool summary / purpose description
- path information / install location / cache location

## 7. Risk Evaluation Model

Deletion advice must have three tiers.

### 7.1 Removable

Definition:

- no strong dependency or reverse-dependency evidence
- removing it usually affects only that tool itself
- recovery cost is low

### 7.2 Keep Recommended

Definition:

- not a strong dependency, but still part of a common workflow
- or has limited reference relationships with manageable impact
- removing it may cause fixable command failures or follow-up work

### 7.3 Core Dependency

Definition:

- strong dependency or reverse-dependency evidence exists
- removing it may affect other tools, runtimes, or workflows
- recovery cost is high, especially for non-technical users

Requirements:

- the detail panel must show the reasoning behind the advice
- wording should remain understandable to non-expert users
- the advice is guidance, not a false guarantee

## 8. Key Interaction Requirements

### 8.1 Startup and Loading

The app must enter a visible loading state immediately at startup.

Requirements:

- no blank freeze
- no “is the app stuck?” feeling
- must include a visible loading animation (spinner or progress bar)
- must show scanning status
- must localize to Chinese on Chinese systems and English on English systems

Current simplified requirement:

- simple copy like “Loading” is acceptable
- overly technical scan logs are not required in the main UI

### 8.2 List Navigation

- support up/down navigation
- support numeric jump
- support multi-select via Space
- the focused row must be clearly visible

### 8.3 Search

- support `/` to enter search mode
- filter results live while typing
- allow quick lookup by tool name
- Esc clears or exits search

### 8.4 Sorting

- support size-based sorting
- sorting state must be visible in the header

### 8.5 Multi-Select and Statistics

When items are selected, the interface must update in real time with:

- selected count
- estimated reclaimable space
- total asset count

### 8.6 Delete

Requirements:

- delete only after selection
- delete must require confirmation
- shortcut should use the macOS-friendly Delete key
- visible feedback is required while deletion is running
- successful deletion should usually update the current list directly rather than forcing a full rescan every time

### 8.7 Cache Cleanup

Requirements:

- cache cleanup only after selection
- must require confirmation
- must show estimated reclaimable space
- should explain that shared caches may be counted once per source

### 8.8 Refresh

- support `R`
- must require confirmation
- confirmation feedback must be visually strong

### 8.9 Quit

- shortcut is `Esc`
- must require confirmation
- must be resistant to accidental exit

## 9. Permission and Password Experience

This is a high-priority UX area.

### 9.1 Principles

- hidden system-level password prompts must not silently appear outside the TUI
- password flow must not feel like a freeze
- password entry, error states, and retries should be handled inside the TUI as much as possible

### 9.2 If Admin Permission Is Required

The UI must:

- clearly explain that admin permission is needed
- clearly explain what the user should do next
- clearly report incorrect passwords and allow retry
- remain visibly active during execution

### 9.3 If a Package Ecosystem Triggers Its Own Lower-Level Permission Flow

Requirements:

- the TUI should take control of the flow as much as possible
- at minimum, the user must never be left wondering whether the app is frozen

## 10. Localization

The app should detect the system language.

### 10.1 Simplified Chinese Systems

- prioritize Simplified Chinese copy
- keep ecosystem names, versions, and unavoidable technical terms in English when needed

### 10.2 English Systems

- use English normally

### 10.3 Minimum Localization Scope

Localization must cover at least:

- top summary area
- footer shortcuts
- loading state
- search hints
- delete / cleanup / refresh / quit confirmations
- permission prompts and errors
- explanation-oriented copy inside the detail panel

## 11. Visual and Layout Goals

This is the area that should be redesigned most heavily.

### 11.1 Problems With the Current TUI

From a product perspective, the current interface still has issues such as:

- unclear information hierarchy
- weak visual rhythm
- action-confirmation area is not strong enough
- some states are not obvious enough
- overall finish still feels incomplete
- functionally capable, but not yet polished as a product

### 11.2 Target for the New Design

The new TUI should:

- feel like an asset manager at first glance
- make the list and detail areas more structurally distinct
- make the top summary area feel more intentional
- make the action area feel like a control surface rather than plain text
- distinguish dangerous, warning, and neutral actions more clearly
- improve transitions between search, selection, confirmation, running, success, and failure states
- fit macOS developer taste while staying terminal-native

## 12. TUI Constraints (Must Be Respected During Design)

This section is critical.

The redesign must respect terminal reality, not assume a web or GUI environment.

### 12.1 Basic Constraints

- runtime environment is a terminal, not a browser
- interaction is keyboard-first
- do not depend on hover
- do not depend on complex modal stacks
- do not depend on real floating windows
- do not require advanced graphics capabilities
- must adapt to common terminal widths
- must handle Chinese and English content gracefully

### 12.2 State Constraints

The design must account for:

- first load
- loaded state
- empty state
- searching
- multi-selected state
- delete confirmation
- cache-clean confirmation
- refresh confirmation
- quit confirmation
- password prompt
- in-progress execution
- success state
- failure state

### 12.3 Feedback Constraints

- any action that may take more than 300ms should have visible feedback
- users should never have to guess whether the program is frozen
- dangerous actions must look dangerous
- error states must be clear and recoverable

## 13. Scope Boundaries

### 13.1 In Scope

- supported-source scanning
- list view
- search
- multi-select
- delete
- cache cleanup
- sorting
- risk evaluation
- Chinese and English localization
- improved permission flow

### 13.2 Out of Scope for This Phase

- blind full-disk filesystem scanning
- arbitrary unsupported ecosystem detection
- mouse-first interaction
- graphical charts
- deep multi-page navigation
- web-style component complexity

## 14. Success Criteria

The redesign is successful if:

- first-time users can understand the app quickly
- users can delete or clean cache without heavy explanation
- users can quickly understand what is large, stale, or risky
- users are not confused by the permission flow
- users do not easily remove the wrong thing or quit by accident
- the overall experience feels like a finished product rather than a prototype

## 15. Priority Design Tasks

The redesign should focus on:

- top summary information architecture
- visual hierarchy of the asset list
- organization of the detail panel
- structure of the action / confirmation area
- search and filter state presentation
- warning and dangerous-action feedback
- loading and in-progress states
- bilingual layout stability

## 16. Expected Design Deliverables

Ideally, the design output should include:

- main layout
- loading state
- search state
- delete confirmation state
- cache-clean confirmation state
- password-entry state
- in-progress state
- empty state
- error state

Bonus if available:

- region responsibilities
- state-transition notes
- shortcut-layout recommendations
- fallback strategy for narrow terminals
