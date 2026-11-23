// Propose a probe page's submissions to the registry on GitHub, for a page no gpu-probe-server serves.
// Each proposal is a file for the registry's submitted/ directory, laid out as the gpu-probe-server crate's submitted
// module reads one, and a link to GitHub's new-file page with it filled in: GitHub forks the repository for a visitor
// who cannot write to it, and opens a pull request from the fork when they press "Propose changes". Each file is a
// pull request of its own.

// Where the files go: the repository, the branch a pull request is opened against, and the directory in it.
const repository = "edge-toolkit/core";
const branch = "main";
const folder = "libs/gpu-probe/signatures/submitted";

// The longest link GitHub opens. A visitor who is not signed in is sent to sign in with the link carried along, which
// answered 500 for links past about 6,100 characters when measured on 2026-11-18, and GitHub refuses any link past
// about 8,000 with 414 whoever follows it. A visit's GPUs are split across as many files as keep each under it.
const longestLink = 6000;

// The longest line a tracked file may hold. A longer one is broken at a space, the rest on a line opening `continued`,
// or where there is no space to break at, between two characters, the rest on a line opening `joined`.
const longestLine = 120;
const continued = "  ";
const joined = "   ";

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

// `text` in ASCII alone, as a tracked file is: a backslash as `\\`, and each character past ASCII, and each control
// character, as `\u{...}`, its code point in hex.
const escaped = (text) => {
  let body = "";
  for (const character of String(text)) {
    const code = character.codePointAt(0);
    if (code === 0x5c) body += "\\\\";
    else if (code > 0x7e || code < 0x20) body += `\\u{${code.toString(16)}}`;
    else body += character;
  }
  return body;
};

// The text `atoms` make.
const textOf = (atoms) => atoms.join("");

// The last space among `atoms` at or before `last` and after `first` that is a single space with a character either
// side, so that breaking there leaves no line ending in a space and no continuation opening with one; or `first`.
const spaceBefore = (atoms, first, last) => {
  for (let index = last; index > first; index -= 1) {
    const single = atoms[index - 1] !== " " && atoms[index + 1] !== undefined && atoms[index + 1] !== " ";
    if (atoms[index] === " " && single) return index;
  }
  return first;
};

// The line `name` then `text`, after a space, broken into lines no longer than the longest. It is broken at a space
// where one fits, and otherwise between two characters short of the longest, never inside an escape and never just
// after a space.
const lineOf = (name, text) => {
  const lines = [];
  // An escape is kept whole, so a break never falls inside one.
  let atoms = `${name} ${text}`.match(/\\u\{[0-9a-f]+\}|\\\\|[^]/gu);
  // The atoms before `first` open a continuation, and are never broken at.
  let first = 0;
  while (textOf(atoms).length > longestLine) {
    let fits = 0;
    let length = 0;
    while (length + atoms[fits].length <= longestLine) {
      length += atoms[fits].length;
      fits += 1;
    }
    const space = spaceBefore(atoms, first, fits);
    if (space > first) {
      lines.push(textOf(atoms.slice(0, space)));
      atoms = [continued, ...atoms.slice(space + 1)];
    } else {
      let cut = fits;
      while (cut > first + 1 && atoms[cut - 1] === " ") cut -= 1;
      lines.push(textOf(atoms.slice(0, cut)));
      atoms = [joined, ...atoms.slice(cut)];
    }
    first = 1;
  }
  lines.push(textOf(atoms));
  return lines.map((each) => `${each}\n`).join("");
};

// A submission's line: its values in output order, or the reason it was withheld.
const submissionLine = (submission) => {
  if (submission.blocked) {
    return lineOf(submission.interface, `blocked: ${escaped(submission.reason)}`);
  }
  return lineOf(submission.interface, Object.values(submission.results).join(" "));
};

// One GPU's block: its label, its row, then a line per submission.
const blockOf = (submissions) => {
  const [first] = submissions;
  const named = Object.keys(first.seen);
  const fields = [
    ...rowOrder.filter((field) => named.includes(field)),
    ...named.filter((field) => !rowOrder.includes(field)),
  ];
  const lines = [
    lineOf("label", escaped(first.label)),
    ...fields.map((field) => lineOf(field, escaped(first.seen[field]))),
    ...submissions.map(submissionLine),
  ];
  return lines.join("");
};

// The text of a file holding `blocks`, a blank line between each two.
const fileOf = (blocks) => blocks.join("\n");

// `submissions` grouped by GPU: those with the same label and row, in the order the first of each was given.
const byGpu = (submissions) => {
  const groups = new Map();
  for (const submission of submissions) {
    const key = JSON.stringify([submission.label, submission.seen]);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(submission);
  }
  return [...groups.values()];
};

// The name a file holding `text` takes: the start of its SHA-256, so a visit proposed twice names the same file, which
// GitHub will not make a second time.
const nameOf = async (text) => {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  const hex = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 16)}.txt`;
};

// Where a proposal's file is made: the registry's own repository and branch, which opens a pull request.
const registry = { repository, branch };

// The link to GitHub's page making the file `name`, holding `text`, in the submitted/ directory of `target`.
const linkTo = (target, name, text) => {
  const query = new URLSearchParams({ filename: `${folder}/${name}`, value: text });
  return `https://github.com/${target.repository}/new/${target.branch}?${query}`;
};

// A name as long as every file's, for measuring a link before its file's name is known.
const anyName = `${"0".repeat(16)}.txt`;

// The longest place a link can make a file in: a fork owned by a 39-character login, the longest GitHub allows, on
// the branch GitHub names for that login's hundredth proposal. A file is measured with it, so its link opens whoever
// follows it.
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
// pull request with it, and whether a link to it is short enough for GitHub to open, which one GPU alone too long for
// it is not. A file holds every submission of each GPU in it, so a GPU is never split across two pull requests.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const proposals = async (submissions) => {
  const files = [];
  let file = [];
  for (const gpu of byGpu(submissions)) {
    const block = { text: blockOf(gpu), count: gpu.length };
    const grown = [...file, block];
    const text = fileOf(grown.map((each) => each.text));
    if (file.length > 0 && linkTo(widest, anyName, text).length > longestLink) {
      files.push(file);
      file = [block];
    } else {
      file = grown;
    }
  }
  if (file.length > 0) files.push(file);
  return Promise.all(
    files.map(async (blocks) => {
      const text = fileOf(blocks.map((block) => block.text));
      const name = await nameOf(text);
      const opens = linkTo(widest, name, text).length <= longestLink;
      const count = blocks.reduce((sum, block) => sum + block.count, 0);
      return { name, text, count, link: linkTo(registry, name, text), opens };
    }),
  );
};
