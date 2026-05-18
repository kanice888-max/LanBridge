# Golden Principles

## Engineering

- Prefer small, verified changes over broad refactors.
- Let existing tests and architecture shape the patch before inventing new abstractions.
- If a review comment repeats, promote it into a test, doc, validation command, or invariant.
- Keep platform differences explicit; do not assume Windows and macOS paths behave the same.
- Prefer scanner correctness before watcher cleverness.
- Prefer a reliable manual/explicit flow before background automation.

## Product

- Data safety beats convenience.
- The UI should reduce path and role confusion.
- Errors should tell the user what action is needed.
- Automatic behavior should never hide destructive or conflict-prone decisions.

## Review

- Review from discovery through ACK/state cleanup, not isolated functions only.
- For sync changes, always ask: what happens on the other device, after restart, and after network failure?
- For UI changes, ask whether a real user on the receiving machine can complete the flow without knowing the sender's filesystem.
