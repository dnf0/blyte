# Contributing to Superstate

Thank you for contributing to Superstate! We welcome contributions that maintain high standards of determinism, correctness, and performance.

## Development Setup

1. Clone the repository:
   ```bash
   git clone https://github.com/climate-x-org/superstate.git
   cd superstate
   ```

2. Run tests across the workspace:
   ```bash
   cargo test --workspace --all-targets
   ```

3. Run the 5-step health check:
   ```bash
   cargo run --bin superstate -- doctor
   ```

## Commit Guidelines

- Follow Conventional Commits (`feat:`, `fix:`, `docs:`, `chore:`, `refactor:`).
- Keep changes atomic and test-driven (write tests first).
- Verify git working tree is clean before finishing development milestones.
