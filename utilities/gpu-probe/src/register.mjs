// Post one submission, given as JSON text, to the server's /register, and say what became of it.
// The answer holds the server's message, and how to show it: "ok", "dup" for one already present, or "bad".
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const register = async (body) => {
  const options = { method: "POST", headers: { "Content-Type": "application/json" }, body };
  try {
    const response = await fetch("/register", options);
    const reply = await response.json();
    const present = reply.message.startsWith("already");
    return { kind: response.ok ? (present ? "dup" : "ok") : "bad", message: reply.message };
  } catch (error) {
    return { kind: "bad", message: `request failed: ${String(error)}` };
  }
};
