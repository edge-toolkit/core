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

// Where a proposal's file is made: the registry's own repository and branch, which opens a pull request, or the
// repository and branch an open pull request was proposed from, which adds the file to that pull request.
const registry = { repository, branch };

// The link to GitHub's page making the file `name`, holding `text`, in the submitted/ directory of `target`.
const linkTo = (target, name, text) => {
  const query = new URLSearchParams({ filename: `${folder}/${name}`, value: text });
  return `https://github.com/${target.repository}/new/${target.branch}?${query}`;
};

// A name as long as every file's, for measuring a link before its file's name is known.
const anyName = `${"0".repeat(16)}.yaml`;

// The longest place a link can make a file in: a fork owned by a 39-character login, the longest GitHub allows, on
// the branch GitHub names for that login's hundredth proposal. A file is measured with it, so its link opens whether
// it opens the pull request or is added to one.
const widest = { repository: `${"x".repeat(39)}/core`, branch: `${"x".repeat(39)}-patch-100` };

// A registry value as text, as the registry reads it: a value its YAML leaves unquoted, such as a browser version,
// is a number in the JSON.
const asText = (value) => (value === undefined || value === null ? undefined : String(value));

// Whether `entry` of the registry is the signature, or the withheld interface, that `submission` gives.
const holds = (entry, submission) => {
  if (entry.interface !== submission.interface || Boolean(entry.blocked) !== Boolean(submission.blocked)) return false;
  if (submission.blocked) return entry.label === submission.label;
  const held = entry.results ?? {};
  const given = Object.entries(submission.results);
  const sameNames = Object.keys(held).length === given.length;
  return sameNames && given.every(([name, value]) => asText(held[name]) === asText(value));
};

// Whether `row`, already in the registry, records `seen`: it gives every field `seen` gives, with the same value.
const covers = (row, seen) => Object.entries(seen).every(([field, value]) => asText(row[field]) === value);

// What the registry `known`, the document signatures.json holds, makes of each of `submissions`: `recorded` when it
// already has the sighting, `sighting` with the entry's label when the signature is known but not seen there, and
// `new` otherwise. These are the answers the server's /register gives the same submission.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const judged = (submissions, known) =>
  submissions.map((submission) => {
    const entry = (known.signatures ?? []).find((candidate) => holds(candidate, submission));
    if (!entry) return { submission, kind: "new" };
    if ((entry.seen ?? []).some((row) => covers(row, submission.seen))) return { submission, kind: "recorded" };
    return { submission, kind: "sighting", label: entry.label };
  });

// The proposals for `submissions`, each submission an object in the shape the server's /register takes: the files
// they are split across, in order, each with its name, its text, how many submissions it holds, the link opening a
// pull request with it, and whether a link to it is short enough for GitHub to open, which one submission alone too
// long for it is not.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const proposals = async (submissions) => {
  const groups = [];
  let group = [];
  for (const submission of submissions) {
    const grown = [...group, item(submission)];
    if (group.length > 0 && linkTo(widest, anyName, fileOf(grown)).length > longestLink) {
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
      const opens = linkTo(widest, name, text).length <= longestLink;
      return { name, text, count: items.length, link: linkTo(registry, name, text), opens };
    }),
  );
};

// The link adding `proposal`'s file to the pull request `pull`, a target `pullOf` gave.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const linkInto = (pull, proposal) => linkTo(pull, proposal.name, proposal.text);

// The repository and branch the open pull request `address` names was proposed from, asked of GitHub's API, which
// answers a page anywhere with no sign-in. It throws, saying why, for anything but an open pull request against the
// registry's repository.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const pullOf = async (address) => {
  const number = address.trim().match(/^https:\/\/github\.com\/([^/]+\/[^/]+)\/pull\/(\d+)/);
  if (!number || number[1].toLowerCase() !== repository) {
    throw new Error(`not a link to a pull request of ${repository}`);
  }
  const response = await fetch(`https://api.github.com/repos/${repository}/pulls/${number[2]}`);
  if (!response.ok) throw new Error(`GitHub answered ${response.status} for pull request ${number[2]}`);
  const pull = await response.json();
  if (pull.state !== "open") throw new Error(`pull request ${number[2]} is ${pull.state}, not open`);
  if (!pull.head?.repo) throw new Error(`the repository pull request ${number[2]} came from is gone`);
  return { repository: pull.head.repo.full_name, branch: pull.head.ref };
};
