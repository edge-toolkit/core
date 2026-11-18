// Propose a probe page's submissions to the registry on GitHub, for a page no gpu-probe-server serves.
// Each proposal is a file for the registry's submitted/ directory, and a link to GitHub's new-file page with it filled
// in: GitHub forks the repository for a visitor who cannot write to it, and opens a pull request from the fork when
// they press "Propose changes".

// Where the files go: the repository, the branch a pull request is opened against, and the directory in it.
const repository = "edge-toolkit/core";
const branch = "main";
const folder = "libs/gpu-probe/signatures/submitted";

// The longest link GitHub opens. A visitor who is not signed in is sent to sign in with the link carried along, which
// answered 500 for links past about 6,100 characters when measured on 2026-11-18, and GitHub refuses any link past
// about 8,000 with 414 whoever follows it. A visit's submissions are split across as many files as keep each under it.
const longestLink = 6000;

// The fields of a row in the order the registry writes them; a field it does not name follows them, in the order given.
const rowOrder = [
  "browser",
  "browser_version",
  "os",
  "os_version",
  "cpu_arch",
  "cpu_version",
  "gpu",
  "gpu_version",
  "driver_version",
  "api_version",
  "backend",
  "device",
  "runtime",
  "defence",
  "reported",
  "window",
  "date",
  "source",
];

// `text` as a YAML double-quoted scalar in ASCII alone, as the registry is: a backslash or double quote (0x5c, 0x22)
// escaped by a backslash, and each character past ASCII, and each control character, as YAML's \u or \U escape for it.
const quoted = (text) => {
  let body = "";
  for (const character of String(text)) {
    const code = character.codePointAt(0);
    if (code === 0x5c || code === 0x22) body += `\\${character}`;
    else if (code > 0xffff) body += `\\U${code.toString(16).padStart(8, "0")}`;
    else if (code > 0x7e || code < 0x20) body += `\\u${code.toString(16).padStart(4, "0")}`;
    else body += character;
  }
  return `"${body}"`;
};

// One submission, in the shape the server's /register takes, as an item of a file's `submissions` list.
const item = (submission) => {
  const lines = [
    `  - interface: ${quoted(submission.interface)}`,
    `    label: ${quoted(submission.label)}`,
    "    seen:",
  ];
  const named = Object.keys(submission.seen);
  const fields = [
    ...rowOrder.filter((field) => named.includes(field)),
    ...named.filter((field) => !rowOrder.includes(field)),
  ];
  for (const field of fields) lines.push(`      ${field}: ${quoted(submission.seen[field])}`);
  if (submission.blocked) {
    lines.push("    blocked: true", `    reason: ${quoted(submission.reason)}`);
  } else {
    lines.push("    results:");
    for (const [name, value] of Object.entries(submission.results)) lines.push(`      ${name}: ${quoted(value)}`);
  }
  return lines.map((line) => `${line}\n`).join("");
};

// The text of a file holding `items`.
const fileOf = (items) => `submissions:\n${items.join("")}`;

// The name a file holding `text` takes: the start of its SHA-256, so a visit proposed twice names the same file, which
// GitHub will not make a second time.
const nameOf = async (text) => {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  const hex = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 16)}.yaml`;
};

// The link to GitHub's page making the file `name`, holding `text`, in the submitted/ directory.
const linkTo = (name, text) => {
  const query = new URLSearchParams({ filename: `${folder}/${name}`, value: text });
  return `https://github.com/${repository}/new/${branch}?${query}`;
};

// A name as long as every file's, for measuring a link before its file's name is known.
const anyName = `${"0".repeat(16)}.yaml`;

// The proposals for `submissions`, each submission an object in the shape the server's /register takes: the files
// they are split across, in order, each with its name, its text, how many submissions it holds, its link, and whether
// the link is short enough for GitHub to open, which one submission alone too long for it is not.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const proposals = async (submissions) => {
  const groups = [];
  let group = [];
  for (const submission of submissions) {
    const grown = [...group, item(submission)];
    if (group.length > 0 && linkTo(anyName, fileOf(grown)).length > longestLink) {
      groups.push(group);
      group = [item(submission)];
    } else {
      group = grown;
    }
  }
  if (group.length > 0) groups.push(group);
  return Promise.all(
    groups.map(async (items) => {
      const text = fileOf(items);
      const name = await nameOf(text);
      const link = linkTo(name, text);
      return { name, text, count: items.length, link, opens: link.length <= longestLink };
    }),
  );
};
