# Clean Code: Structure Rules

## Control Flow

**No complex nesting.** Nested `if/else` chains are hard to read and hard to maintain.
Replace them with:
- **Dictionary / table dispatch**: map keys to behaviors, look them up at runtime
- **Early returns / guard clauses**: handle the exceptional path first and return, keeping the happy path at the lowest indentation level
- **Polymorphism**: if branching on type, consider a trait/interface instead

## Functions and Parameters

**Multi-parameter functions are a smell.** When a function takes many parameters, group them into a parameter object / struct. This makes call sites readable, enables named construction, and makes future additions backward-compatible.

**Helper functions belong close to their caller.** If a small function exists only to reduce the complexity of one other function, define it as a nested / inner function. This limits its scope and signals that it has no broader purpose.

## Types: Structs vs. Classes / Objects

Distinguish between *data containers* and *behavior carriers*:

| Concept | All fields public | Created by filling fields | Invariants enforced |
|---------|------------------|--------------------------|---------------------|
| Struct  | ✅ | ✅ | ❌ |
| Class / Object | ❌ (members private) | ❌ | ✅ (constructor only) |

- Class members must be private; external code must not set them directly.
- Objects must be created through constructors, not by assigning fields one by one.
- Complex construction logic should use the **Builder pattern** rather than a constructor with many optional parameters.

## Null / Optional Handling

When an object is conceptually optional but callers don't need to care, use a **Null Object / Dummy Object** that implements the same interface and does nothing. This eliminates `null` checks at every call site. Avoid wrapping objects in `Option`/`Optional`/nullable wrappers and then passing those wrappers around — that leaks the optionality into every consumer.

## Error Handling

Draw a sharp line between two kinds of failures:

| Kind | Meaning | Response |
|------|---------|----------|
| Logic error | A precondition was violated; the program is in an impossible state | `panic` / `assert` / crash immediately |
| Environmental error | Network down, disk full, permission denied | Handle gracefully; do not terminate the process |

Do not introduce `unreachable` branches, sentinel values, or exhaustiveness checks just to satisfy a compiler when the branch is logically impossible. Instead, restructure the types so the impossible state cannot be represented.

## Testing

Never add `#[cfg(test)]` blocks or `if TEST_MODE` branches to production code paths.
Achieve testability through:
- **Mock objects** for dependencies
- **I/O separation**: write pure functions that accept data, then a thin I/O layer that feeds them
- **Dependency injection**: pass interfaces/traits rather than concrete implementations

### No Meaningless Tests

A test must verify real behavior. Flag and delete tests that prove nothing, such as:

- **Field-existence tests**: asserting that a struct has a particular field (the compiler already guarantees this)
- **Default-value tautologies**: constructing a value with `Default::default()` / `new()` and then asserting that each field equals its default — this only tests that the author typed the default value correctly, not that the code does anything useful
- **Getter/setter round-trips** with no logic in between
- **Empty tests**: test functions whose body is just `// TODO` or a single `assert!(true)`

A useful test describes a *scenario* (given some input or state), exercises *behavior* (calls a function or method), and asserts a *meaningful outcome* (the result is correct, an error is returned under the right condition, a side effect occurred).

**Example of a meaningless test to delete:**

```rust
#[test]
fn test_user_default() {
    let u = User::default();
    assert_eq!(u.name, "");
    assert_eq!(u.age, 0);
}
```

This test adds zero value. Delete it.

## Resource Management

Bind every resource (file handle, socket, thread, lock, GPU context, …) to an owning object. The resource must be acquired in the constructor and released in the destructor / `Drop` implementation. Do not manage resource lifetimes manually outside of ownership structures.

## Review Actions

For each violation found, produce a concrete, targeted change: show the problematic code and the restructured replacement.
