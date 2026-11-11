import { createRoot } from "react-dom/client";
import { post, update, type View } from "./bridge";
import { App } from "./app/App";
import { paste } from "./app/editing";
import { forwardShortcuts, refuseDrops, typeIntoComposer } from "./app/shortcuts";
import "./styles/index.css";

window.spooky = { update: (view) => update(view as View), focusComposer: () => {}, paste };
forwardShortcuts();
typeIntoComposer();
refuseDrops();

const root = document.getElementById("root");
if (root) createRoot(root).render(<App />);
post({ type: "ready" });
