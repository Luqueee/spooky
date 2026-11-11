# Agent view

The conversation view Spooky shows for an agent pane on macOS, in a web view. It is built to
one self-contained file, `dist/index.html`, which the desktop app embeds. That file is
committed, so `cargo build` needs no Node; CI rebuilds it and fails if it differs.

```
npm ci            # once
npm run check     # type check and unit tests
npm run build     # writes dist/index.html
npm run dev       # in a browser, with actions logged to the console
```

## Layout

```
src/
  main.tsx              entry: wires the bridge, renders <App/>
  app/                  the shell: App, Footer, shortcut forwarding
  bridge/               everything that crosses to the desktop app
    types.ts            the agent as the server describes it (spooky_core::Agent)
    ipc.ts              the closed set of actions the page may post
    store.ts            the state the app pushes, as a React store
  features/             one folder per part of the view
    conversation/       entries, tool cards, folded reasoning
    markdown/           answers, code blocks, highlighting
    composer/           the multiline input and its per-pane drafts
    dialogs/            questions and approvals; only a click answers them
    models/             model and thinking lists, searching and roles
    usage/              the remaining-usage panel
  ui/                   small pieces shared by features
  styles/               one stylesheet per area, imported by index.css
```

Dependency direction: `app` → `features` → `ui` and `bridge`. Features do not import each
other, except that `conversation` draws `markdown`.

## Rules

- State lives in Rust. The page holds only what is purely visual: folds, an open list, a
  draft. Every change of the agent goes out as an action and comes back as state.
- What the agent wrote is untrusted. Markdown is rendered without raw HTML or images, links
  only report themselves, and the Content-Security-Policy in `index.html` allows no network.
- Actions are validated again by the desktop app (`features/agent/web.rs`); keep `ipc.ts`
  and that file in step.
