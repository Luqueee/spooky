import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import go from "highlight.js/lib/languages/go";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

const languages = {
  bash, c, cpp, css, diff, go, java, javascript, json, kotlin, markdown, python, ruby, rust,
  sql, swift, typescript, xml, yaml,
};
for (const [name, definition] of Object.entries(languages)) hljs.registerLanguage(name, definition);
const aliases: Record<string, string> = {
  sh: "bash", shell: "bash", zsh: "bash", js: "javascript", jsx: "javascript", ts: "typescript",
  tsx: "typescript", py: "python", rs: "rust", html: "xml", yml: "yaml", "c++": "cpp",
  patch: "diff", md: "markdown", kt: "kotlin", rb: "ruby", jsonc: "json", toml: "yaml",
};

/** The highlighted HTML of `code` (already escaped by the highlighter), or null when the
 * language is not one of the registered ones. */
export function highlighted(code: string, language: string | undefined): string | null {
  if (!language) return null;
  const name = aliases[language.toLowerCase()] ?? language.toLowerCase();
  if (!hljs.getLanguage(name)) return null;
  return hljs.highlight(code, { language: name, ignoreIllegals: true }).value;
}
