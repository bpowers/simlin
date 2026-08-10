# Development Workflow

## Problem-Solving Philosophy

- **Write high-quality, general-purpose solutions**: Implement solutions that work correctly for all valid inputs, not just test cases. Do not hard-code values or create solutions that only work for specific test inputs.
- **Prioritize the right approach over the first approach**: Research the proper way to implement features rather than implementing workarounds. If unsure, explore several approaches and then choose the most promising one (or ask the user for their input if one isn't clearly best).
- **Keep implementations simple and maintainable**: Start with the simplest solution that meets requirements. Only add complexity when the simple approach demonstrably fails.
- **No special casing in tests**: Tests should hold all implementations to the same standard. Never add conditional logic in tests that allows certain implementations to skip requirements.
- **No compatibility shims or fallback paths**: There are no external users of this codebase, and we have a comprehensive test suite. Fully complete migrations.
- **Test-driven Development (TDD)**: Follow TDD best practices; ensure tests actually assert the behavior we're expecting AND have high code coverage.

## Understanding Requirements

- Read relevant code and documentation (including for libraries) and build a plan based on the task.
- If there are important and ambiguous high-level architecture decisions or "trapdoor" choices, stop and ask the user.
- Start by adding tests to validate assumptions before making changes.
- Build the simplest interfaces and abstractions possible while fully addressing the task in full generality.

## One Concept, One Owner

Before creating a parser, manifest type, constant, fixture builder, policy check, or command, find the existing owner of the concept and extend it. Every duplicate owner is a second plausible precedent -- code is read as prompt material by both humans and agents, and each variation forces the next reader to research which one is canonical. Warning signs that a concept has lost its single owner:

- a second parser for a format the repository already models;
- a fact (a version, a threshold, a path) copied into policy code, fixtures, or tests instead of read from its canonical source;
- structured output (JSON, XML, protobuf text) assembled from string fragments instead of serialized from the owning type;
- untyped external data flowing past the boundary that should have decoded it once into a domain type; and
- several defensive checks scattered across call sites all guarding the same invariant -- evidence that no type, boundary, or state machine owns it. Ask what owner would make the invalid state unrepresentable, rather than adding another check.

A related distinction keeps policy maintainable: **current state is data, durable policy is code**. A version pin, an inventory entry, or a tool selection lives in its canonical manifest, and checks parse that manifest rather than repeating the value; the durable relationship (exact pinning, an approved dependency class, a required agreement between two files) is what belongs in code. A routine bump that requires editing constants and fixtures across the tree means the policy was modeled backwards.

## Responding to Feedback

If you get feedback on code that you don't think is actionable, it at a minimum indicates you are missing comments providing appropriate context for why the code looks that way or does what it does.
