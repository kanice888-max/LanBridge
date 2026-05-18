# Cleanup Workflow

Cleanup must preserve behavior and user data safety.

## When To Cleanup

- A repeated review finding points at the same pattern.
- A module boundary is confusing enough to cause implementation mistakes.
- A warning or unused path hides a real behavior gap.
- Documentation no longer matches the shipped flow.

## How To Cleanup

1. Write down the behavior that must not change.
2. Add or identify a test that protects it.
3. Make a narrow cleanup patch.
4. Run targeted validation.
5. Record remaining risk in `docs/quality/debt-log.md`.

## Avoid

- Broad rewrites during release stabilization.
- Renaming product concepts without updating UI copy, docs, and tests.
- Removing safety checks because they look redundant.
- Cleaning one platform while leaving the other with divergent shared behavior.
