//! The agent loop: superstate owning execution rather than wrapping someone else's.
//!
//! Every mechanism this project built to influence an agent from outside its loop was capped or
//! negative. A PreToolUse hook cannot prevent the call it intercepts, because the model has
//! already emitted the request and the conversation has already been re-sent by the time the
//! hook runs (FINDINGS 13.25). Refusing a read the worker had decided it needed simply produced
//! a second request: 89-91% of suppressions were immediately insisted on. And 47.6% of all
//! input tokens went to turns whose entire output was "waiting for the background task",
//! because the host harness backgrounded slow commands and the worker polled for them, each
//! poll costing a full turn carrying the whole conversation (13.29).
//!
//! None of those are fixable from outside. All of them are design decisions from inside.

pub mod state;
pub mod tools;
