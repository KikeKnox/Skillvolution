// skillvolution-managed:opencode-plugin
// Skillvolution for OpenCode: injects the vault catalog into the system prompt
// and asks for the evolution review once the agent goes idle after doing work.
// Setup rewrites this file on install; local edits will be overwritten.
import { execFile } from "node:child_process";

const BIN = __SKILLVOLUTION_BIN__;
const DB = __SKILLVOLUTION_DB__;

// Keep in sync with STOP_REASON in src/hook.rs.
const REVIEW_PROMPT =
  "Skillvolution review: you changed files or ran commands since the last review. " +
  "Follow the Report and Reflect steps of the evolution skill now: call report_skill_outcome for any vault skill you applied, " +
  "and call propose_skill_change only for a lesson that meets every lesson criterion. " +
  'If there is nothing to report or propose, reply only "No lesson." and stop.';

// OpenCode built-in tool ids that change files or run commands.
const WORK_TOOLS = new Set(["edit", "write", "multiedit", "patch", "apply_patch", "bash"]);
// MCP tools are exposed as `<server>_<tool>`, so match by substring.
const REVIEW_TOOLS = ["propose_skill_change", "report_skill_outcome"];

export const SkillvolutionPlugin = async ({ client, directory }) => {
  const log = (level, message) =>
    client.app
      .log({ body: { service: "skillvolution", level, message } })
      .catch(() => {});

  const catalogs = new Map(); // sessionID -> Promise<string>
  const sessions = new Map(); // sessionID -> { worked, reviewed, reviewing, agent, model }
  const state = (id) => {
    if (!sessions.has(id)) sessions.set(id, { worked: false, reviewed: false, reviewing: false });
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

  const parentOf = async (sessionID) => {
    try {
      const { data } = await client.session.get({ path: { id: sessionID } });
      return data?.parentID;
    } catch {
      return undefined;
    }
  };

  const onIdle = async (sessionID) => {
    const s = sessions.get(sessionID);
    if (!s) return;
    const needsReview = s.worked && !s.reviewed && !s.reviewing;
    s.worked = s.reviewed = s.reviewing = false; // the turn caused by the review prompt never re-triggers
    if (!needsReview) return;

    // Subagent (task) sessions hand their work to the parent instead of being prompted.
    const parentID = await parentOf(sessionID);
    if (parentID) {
      state(parentID).worked = true;
      return;
    }

    s.reviewing = true;
    try {
      await client.session.promptAsync({
        path: { id: sessionID },
        body: {
          agent: s.agent,
          model: s.model,
          parts: [{ type: "text", text: REVIEW_PROMPT, synthetic: true }],
        },
      });
    } catch (error) {
      s.reviewing = false;
      log("error", `review prompt failed: ${error?.message ?? error}`);
    }
  };

  return {
    "experimental.chat.system.transform": async ({ sessionID }, output) => {
      if (!sessionID) return; // e.g. agent generation outside a session
      if (!catalogs.has(sessionID)) catalogs.set(sessionID, loadCatalog());
      const catalog = await catalogs.get(sessionID);
      if (catalog) output.system.push(catalog);
    },

    "chat.message": async ({ sessionID, agent, model }) => {
      const s = state(sessionID);
      s.agent = agent;
      s.model = model;
    },

    "tool.execute.after": async ({ tool, sessionID }) => {
      const s = state(sessionID);
      if (WORK_TOOLS.has(tool)) s.worked = true;
      if (REVIEW_TOOLS.some((name) => tool.includes(name))) s.reviewed = true;
    },

    event: async ({ event }) => {
      const sessionID = event.properties?.sessionID ?? event.properties?.info?.id;
      if (!sessionID) return;
      if (event.type === "session.idle") void onIdle(sessionID); // don't hold up event delivery
      // Aborted or failed turns are not reviewed; the idle that follows sees a clean span.
      else if (event.type === "session.error") {
        const s = sessions.get(sessionID);
        if (s && !s.reviewing) s.worked = s.reviewed = false;
      } else if (event.type === "session.deleted") {
        sessions.delete(sessionID);
        catalogs.delete(sessionID);
      }
    },
  };
};
