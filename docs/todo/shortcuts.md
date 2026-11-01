# Custom keyboard shortcuts

**When:** not decided.

`SHORTCUTS` is a fixed table. Preferences store no binding.

**Why:**

- Editors who switch programs want their own keys.
- The default for each action stays the Cut page key ([spec §11](../cut-page-spec.md#11-keyboard-shortcuts)).
- A script calls a request.

**To do:**

- Preferences store one binding for each action in `SHORTCUTS`.
- A missing binding means the default key.
- `keys.list` reports the bindings in force.
- Help and the menus show those keys.
- One key belongs to one action.
- The preferences window names the action that already uses a chosen key.
- A request sets a binding, and a request resets one binding or all of them.
