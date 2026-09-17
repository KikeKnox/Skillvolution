// skillvolution-managed:opencode-plugin
// Skillvolution for OpenCode: injects the vault catalog into the system prompt
// and, once a turn that did work ends without a report or publication, adds a
// review reminder to the system prompt of the following turns until the work
// is reviewed. It never prompts the session itself, so no extra turn is spent.
// Setup rewrites this file on install; local edits will be overwritten.
import { execFile } from "node:child_process";

const BIN = __SKILLVOLUTION_BIN__;
const DB = __SKILLVOLUTION_DB__;

// Keep in sync with STOP_REASON in src/hook.rs.
const REVIEW_REMINDER =
  "Skillvolution review: this session did work that has not been reviewed yet. " +
  "Before finishing, follow the Report and Reflect steps of the evolution skill: " +
  "call report_skill_outcome for any vault skill you applied, and for any candidate " +
  "lesson that meets every lesson criterion, dispatch a fresh subagent now — " +
  "without asking the user — to judge it " +
  "(global scope, project scope, or discard), then call publish_skill with the verdict.";

// OpenCode built-in tool ids that change files or run commands.
const WORK_TOOLS = new Set(["edit", "write", "multiedit", "patch", "apply_patch", "bash"]);
// MCP tools are exposed as `<server>_<tool>`, so match by substring.
const REVIEW_TOOLS = ["publish_skill", "report_skill_outcome"];

export const SkillvolutionPlugin = async ({ client, directory }) => {
  const log = (level, message) =>
    client.app
      .log({ body: { service: "skillvolution", level, message } })
      .catch(() => {});

  const catalogs = new Map(); // sessionID -> Promise<string>
  const sessions = new Map(); // sessionID -> { worked, reviewed, remind }
  const parents = new Map(); // sessionID -> Promise<parentID | undefined>
  const state = (id) => {
    if (!sessions.has(id)) sessions.set(id, { worked: false, reviewed: false, remind: false });
    return sessions.get(id);
  };

  // The binary detects the project from cwd; a CLAUDE_PROJECT_DIR inherited
  // from an enclosing Claude Code would override that, so drop it.
  const env = { ...process.env };
  delete env.CLAUDE_PROJECT_DIR;

  const loadCatalog = () =>
    new Promise((resolve) => {
      const options = { cwd: directory, env, timeout: 15000 };
      execFile(BIN, ["--db", DB, "hook", "session-start"], options, (error, stdout) => {
        if (error) {
          log("warn", `catalog unavailable: ${error.message}`);
          return resolve("");
        }
        resolve(stdout.trim());
      });
    });

  // Subagent (task) sessions hand their work and reviews to the parent:
  // flags always live on the top-level session so delegated edits still get
  // reviewed and subagent context is never polluted with the reminder.
  const parentOf = (id) => {
    if (!parents.has(id)) {
      parents.set(
        id,
        client.session
          .get({ path: { id } })
          .then(({ data }) => data?.parentID)
          .catch(() => undefined),
      );
    }
    return parents.get(id);
  };
  const ownerOf = async (id) => (await parentOf(id)) ?? id;

  return {
    "experimental.chat.system.transform": async ({ sessionID }, output) => {
      if (!sessionID) return; // e.g. agent generation outside a session
      if (!catalogs.has(sessionID)) catalogs.set(sessionID, loadCatalog());
      const catalog = await catalogs.get(sessionID);
      if (catalog) output.system.push(catalog);
      if (sessions.get(sessionID)?.remind) output.system.push(REVIEW_REMINDER);
    },

    "tool.execute.after": async ({ tool, sessionID }) => {
      const s = state(await ownerOf(sessionID));
      if (WORK_TOOLS.has(tool)) s.worked = true;
      if (REVIEW_TOOLS.some((name) => tool.includes(name))) {
        s.reviewed = true;
        s.remind = false;
      }
    },

    event: async ({ event }) => {
      const sessionID = event.properties?.sessionID ?? event.properties?.info?.id;
      if (!sessionID) return;
      if (event.type === "session.idle" || event.type === "session.error") {
        const s = sessions.get(await ownerOf(sessionID));
        if (!s) return;
        // Idle: a turn that did work without reviewing earns a reminder on the
        // following turns; each span is judged once. Error: aborted or failed
        // turns are not reviewed; a pending reminder survives either way.
        if (event.type === "session.idle" && s.worked && !s.reviewed) s.remind = true;
        s.worked = s.reviewed = false;
      } else if (event.type === "session.deleted") {
        sessions.delete(sessionID);
        catalogs.delete(sessionID);
        parents.delete(sessionID);
      }
    },
  };
};
