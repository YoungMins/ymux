// Remote server, app side: answers the backend's `remote://request`, shows
// pairing requests and keeps the phone's pane list current. Started once from
// main.ts (not from Settings, which may never be opened).

import { t } from "../i18n/i18n";
import {
  onAutoAnswer,
  onRemotePairConflict,
  onRemotePairRequest,
  onRemoteRequest,
  onRemoteStatus,
  remoteApi,
} from "../ipc/bridge";
import type { RemoteStatus } from "../types";
import { showToast } from "../ui/Toast";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { bumpInput } from "./inputGen";
import { askPairRequest, dismissPairRequest } from "./pairDialog";
import { handleRequest } from "./remoteOps";
import { buildPaneList, shouldPublish } from "./remoteModel";
import { createPublisher } from "./remotePublisher";

export function startRemote(manager: WorkspaceManager): () => void {
  const host = manager.remoteHost();
  let status: RemoteStatus | null = null;

  const publisher = createPublisher({
    compute: () => buildPaneList(host.listInput()),
    send: (panes) => remoteApi.publishPanes(panes),
    active: () => shouldPublish(status?.state),
  });

  const refresh = async (): Promise<void> => {
    const before = status?.state;
    try {
      status = await remoteApi.status();
    } catch {
      status = null;
    }
    // A fresh connection has not seen the list yet.
    if (shouldPublish(status?.state) && before !== status?.state) publisher.invalidate();
  };

  // Panes that are gone lose their menu ids. (The agent's raw status
  // generation is fed by `WorkspaceManager.applyAgents`, per snapshot.)
  const onChange = (): void => {
    publisher.poke();
    if (!shouldPublish(status?.state)) return;
    try {
      host.menus.keepOnly(buildPaneList(host.listInput()).map((p) => p.pane));
    } catch (e) {
      console.warn("remote menu cleanup failed:", e);
    }
  };
  const offs: (() => void)[] = [manager.onTreeChange(onChange), manager.onInboxChange(onChange)];
  let disposed = false;
  const keep = (p: Promise<() => void>, what: string): void => {
    p.then((off) => (disposed ? off() : offs.push(off))).catch((e) =>
      console.warn(`${what} listen failed:`, e),
    );
  };

  keep(
    onRemoteRequest((req) => {
      void (async () => {
        const data = await handleRequest(host, req);
        await remoteApi.reply(req.id, data).catch((e) => console.warn("remote_reply failed:", e));
      })();
    }),
    "remote://request",
  );
  keep(
    onRemoteStatus(() => void refresh()),
    "remote://status",
  );
  // An automatic answer typed into a pane is input too (the backend answers
  // the agent's hook, not the terminal, so it never passes `writePane`).
  keep(
    onAutoAnswer((ev) => bumpInput(ev.paneId)),
    "auto-continue:answered",
  );

  // One dialog at a time, in arrival order. The dialog stays open across
  // wrong numbers (the backend counts them and says how many tries remain).
  // A conflict (two devices used the code) ends the window: the open dialog
  // goes away, requests still queued behind it are skipped (the backend
  // already refused them), and the user is told nothing was paired.
  let chain: Promise<void> = Promise.resolve();
  let epoch = 0;
  keep(
    onRemotePairRequest((req) => {
      const mine = epoch;
      chain = chain.then(async () => {
        if (mine !== epoch) return;
        const end = await askPairRequest(req.name, (cap, check) =>
          remoteApi.pairDecide(req.device_id, true, cap, check),
        );
        if (mine !== epoch) return;
        if (end === "refused") {
          showToast(t("settings.remote.pairReq.refused"), undefined, 20000);
        } else if (end === "error") {
          showToast(t("settings.remote.pairReq.failed"), undefined, 10000);
        } else if (end === "dismissed") {
          // Reject, Esc or a click outside: never an approval by accident.
          try {
            await remoteApi.pairDecide(req.device_id, false, "view", "");
          } catch (e) {
            console.warn("remote_pair_decide failed:", e);
          }
        }
      });
    }),
    "remote://pair-request",
  );
  keep(
    onRemotePairConflict(() => {
      epoch++;
      dismissPairRequest();
      showToast(t("settings.remote.pairConflict"), undefined, 20000);
    }),
    "remote://pair-conflict",
  );

  void refresh();

  return () => {
    disposed = true;
    publisher.dispose();
    for (const off of offs) off();
  };
}
