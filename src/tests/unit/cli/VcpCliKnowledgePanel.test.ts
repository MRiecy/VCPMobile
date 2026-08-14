import { beforeEach, describe, expect, it, vi } from "vitest";
import VcpCliManifestView from "@/features/cli/components/VcpCliManifestView.vue";
import { useOverlayStore } from "@/core/stores/overlay";
import {
  VCP_CLI_ACTION_COMMAND,
  VCP_CLI_KNOWLEDGE_CATALOG_COMMAND,
  VCP_CLI_KNOWLEDGE_IMPORT_COMMIT_COMMAND,
  VCP_CLI_KNOWLEDGE_IMPORT_DISCARD_COMMAND,
  VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND,
  VCP_CLI_KNOWLEDGE_REVOKE_COMMAND,
  VCP_CLI_NATIVE_PICK_FILE_COMMAND,
  VCP_CLI_STATUS_COMMAND,
  type VcpCliActionResponse,
  type VcpCliKnowledgeCatalog,
  type VcpCliKnowledgeImportCandidate,
  type VcpCliResultBody,
  type VcpCliRuntimeStatus,
  useVcpCliStore,
} from "@/features/cli/vcpCliStore";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import { flushPromises } from "@/tests/utils/flush";
import { mountWithPinia } from "@/tests/utils/mount";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const readyStatus: VcpCliRuntimeStatus = {
  available: true,
  availability_reason: null,
  background_reliability: "foreground_only",
  runtime_generation: 1,
  phase: "ready",
  profile_id: "alpine-arm64-v1",
  max_concurrent_jobs: 2,
  running_jobs: 0,
  jobs: [],
};

function actionSuccess(
  operationId: string,
  result: Partial<VcpCliResultBody> = {},
): VcpCliActionResponse {
  return {
    operation_id: operationId,
    runtime_generation: 1,
    envelope: {
      status: "success",
      result: { content: [], ...result },
    },
  };
}

function candidate(
  patch: Partial<VcpCliKnowledgeImportCandidate> = {},
): VcpCliKnowledgeImportCandidate {
  return {
    token: "vcp-knowledge-candidate:opaque-1",
    candidate_sha256: "a".repeat(64),
    catalog_generation: 4,
    display_name: "notes.md",
    mime_type: "text/markdown",
    size_bytes: 2048,
    index_text_truncated: false,
    chunk_count: 3,
    used_bytes: 4096,
    limit_bytes: 512 * 1024 * 1024,
    pending_used_bytes: 2048,
    pending_limit_bytes: 128 * 1024 * 1024,
    warnings: [],
    replayed: false,
    ...patch,
  };
}

function catalog(
  patch: Partial<VcpCliKnowledgeCatalog> = {},
): VcpCliKnowledgeCatalog {
  return {
    schema_version: 1,
    catalog_generation: 4,
    used_bytes: 4096,
    limit_bytes: 512 * 1024 * 1024,
    pending_used_bytes: 0,
    pending_limit_bytes: 128 * 1024 * 1024,
    active_source_count: 1,
    active_source_limit: 64,
    pending_candidate_count: 0,
    pending_candidate_limit: 16,
    sources: [
      {
        source_id: "knowledge-source-1",
        display_name: "guide.md",
        mime_type: "text/markdown",
        size_bytes: 4096,
        source_sha256: "b".repeat(64),
        index_status: "ready",
        index_text_truncated: false,
        chunk_count: 5,
        granted_at_ms: 1_786_600_000_000,
      },
    ],
    ...patch,
  };
}

function mountView() {
  return mountWithPinia(VcpCliManifestView, {
    props: { isOpen: true, zIndex: 44 },
  });
}

async function openKnowledgeTab(
  wrapper: ReturnType<typeof mountView>,
): Promise<void> {
  await wrapper.get('[data-vcp-cli-tab="knowledge"]').trigger("click");
  await flushPromises();
}

describe("VCP CLI local knowledge grant UI", () => {
  beforeEach(() => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () => readyStatus);
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = args?.request as { operation_id: string };
      return actionSuccess(request.operation_id, { jobs: [] });
    });
    mockInvoke(VCP_CLI_KNOWLEDGE_CATALOG_COMMAND, () => catalog());
  });

  it("keeps local knowledge in the same four-tab SlidePage and lets Rust own the picker", async () => {
    const inspectCandidate = candidate({
      index_text_truncated: true,
      warnings: ["文件尾部不会进入索引。"],
    });
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND, (args) => ({
      operation_id: (args?.request as { operation_id: string }).operation_id,
      status: "candidate",
      candidate: inspectCandidate,
    }));
    const wrapper = mountView();
    await flushPromises();

    expect(wrapper.findAll("[data-vcp-cli-tab]")).toHaveLength(4);
    await openKnowledgeTab(wrapper);
    expect(wrapper.text()).toContain("本机授权 catalog");
    expect(wrapper.text()).toContain("guide.md");
    expect(
      wrapper.get('[data-vcp-cli-role="knowledge-quota"]').text(),
    ).toContain("1 / 64");

    await wrapper
      .get('[data-vcp-cli-action="inspect-knowledge-import"]')
      .trigger("click");
    await flushPromises();

    const inspectCall = invokeMock.mock.calls.find(
      ([command]) => command === VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND,
    );
    expect(inspectCall?.[1]).toEqual({
      request: {
        operation_id: expect.stringMatching(/^vcp-cli-knowledge-inspect-/),
      },
    });
    expect(JSON.stringify(inspectCall?.[1])).not.toMatch(/path|uri|staging/i);
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === VCP_CLI_NATIVE_PICK_FILE_COMMAND,
      ),
    ).toBe(false);
    expect(
      wrapper.get('[data-vcp-cli-role="knowledge-import-review"]').text(),
    ).toContain("当前只是候选，尚未授权");
    expect(wrapper.text()).toContain("索引文本受 1 MiB 上限截断");
    expect(wrapper.text()).toContain("文件尾部不会进入索引");
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === VCP_CLI_KNOWLEDGE_IMPORT_COMMIT_COMMAND,
      ),
    ).toBe(false);
    wrapper.unmount();
  });

  it("treats native picker cancellation as a bounded success without a grant", async () => {
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND, (args) => ({
      operation_id: (args?.request as { operation_id: string }).operation_id,
      status: "cancelled",
    }));
    const wrapper = mountView();
    await openKnowledgeTab(wrapper);

    await wrapper
      .get('[data-vcp-cli-action="inspect-knowledge-import"]')
      .trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("已取消文件选择，未创建授权");
    expect(
      wrapper.find('[data-vcp-cli-role="knowledge-import-review"]').exists(),
    ).toBe(false);
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === VCP_CLI_KNOWLEDGE_IMPORT_COMMIT_COMMAND,
      ),
    ).toBe(false);
    wrapper.unmount();
  });

  it("renders a deterministic display-name and source-id order", async () => {
    const base = catalog().sources[0];
    mockInvoke(VCP_CLI_KNOWLEDGE_CATALOG_COMMAND, () =>
      catalog({
        active_source_count: 3,
        sources: [
          { ...base, source_id: "source-z", display_name: "zeta.md" },
          { ...base, source_id: "source-b", display_name: "alpha.md" },
          { ...base, source_id: "source-a", display_name: "alpha.md" },
        ],
      }),
    );
    const wrapper = mountView();
    await openKnowledgeTab(wrapper);

    expect(
      wrapper
        .findAll('[data-vcp-cli-role="knowledge-row"]')
        .map((row) => row.text()),
    ).toEqual([
      expect.stringContaining("source-a"),
      expect.stringContaining("source-b"),
      expect.stringContaining("source-z"),
    ]);
    wrapper.unmount();
  });

  it("requires the full disclosure confirmation before commit and rescans backend truth", async () => {
    let catalogRead = 0;
    mockInvoke(VCP_CLI_KNOWLEDGE_CATALOG_COMMAND, () => {
      catalogRead += 1;
      return catalogRead === 1
        ? catalog({ sources: [], active_source_count: 0, used_bytes: 0 })
        : catalog({ catalog_generation: 5 });
    });
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND, (args) => ({
      operation_id: (args?.request as { operation_id: string }).operation_id,
      status: "candidate",
      candidate: candidate(),
    }));
    const commits: Array<Record<string, unknown>> = [];
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_COMMIT_COMMAND, (args) => {
      const request = args?.request as Record<string, unknown>;
      commits.push(request);
      return {
        operation_id: request.operation_id,
        catalog_generation: 5,
        replayed: false,
        source: catalog().sources[0],
      };
    });
    const wrapper = mountView();
    await openKnowledgeTab(wrapper);
    await wrapper
      .get('[data-vcp-cli-action="inspect-knowledge-import"]')
      .trigger("click");
    await flushPromises();

    await wrapper
      .get('[data-vcp-cli-action="commit-knowledge-import"]')
      .trigger("click");
    const overlay = useOverlayStore();
    expect(commits).toHaveLength(0);
    expect(overlay.confirmConfig?.title).toBe("复制并授权本机知识？");
    expect(overlay.confirmConfig?.message).toContain("App 私有知识库");
    expect(overlay.confirmConfig?.message).toContain("localLoopback vref");
    expect(overlay.confirmConfig?.message).toContain("目标 CLI Job");
    expect(overlay.confirmConfig?.message).toContain("stdout/stderr");
    expect(overlay.confirmConfig?.message).toContain("当前选择的模型服务");
    expect(overlay.confirmConfig?.message).toContain(
      "不是聊天附件、同步数据或提示词",
    );
    expect(overlay.confirmConfig?.message).toContain(
      "Agent 不能选择 catalog 外文件",
    );
    overlay.confirmConfig?.onCancel();
    await flushPromises();
    expect(commits).toHaveLength(0);
    expect(
      wrapper.find('[data-vcp-cli-role="knowledge-import-review"]').exists(),
    ).toBe(true);

    await wrapper
      .get('[data-vcp-cli-action="commit-knowledge-import"]')
      .trigger("click");
    overlay.confirmConfig?.onConfirm();
    await vi.waitFor(() => expect(catalogRead).toBe(2));
    await flushPromises();

    expect(commits).toHaveLength(1);
    expect(commits[0]).toMatchObject({
      operation_id: expect.stringMatching(/^vcp-cli-knowledge-commit-/),
      token: "vcp-knowledge-candidate:opaque-1",
      candidate_sha256: "a".repeat(64),
      expected_catalog_generation: 4,
    });
    expect(wrapper.text()).toContain("guide.md");
    wrapper.unmount();
  });

  it("keeps the source on revoke failure and reuses the same operation id on retry", async () => {
    const revokeRequests: Array<Record<string, unknown>> = [];
    let revokeAttempt = 0;
    let revoked = false;
    mockInvoke(VCP_CLI_KNOWLEDGE_CATALOG_COMMAND, () =>
      revoked
        ? catalog({
            catalog_generation: 5,
            sources: [],
            active_source_count: 0,
            used_bytes: 0,
          })
        : catalog(),
    );
    mockInvoke(VCP_CLI_KNOWLEDGE_REVOKE_COMMAND, (args) => {
      const request = args?.request as Record<string, unknown>;
      revokeRequests.push(request);
      revokeAttempt += 1;
      if (revokeAttempt === 1) throw new Error("catalog write failed");
      revoked = true;
      return {
        operation_id: request.operation_id,
        catalog_generation: 5,
        replayed: true,
        source_id: "knowledge-source-1",
        deletion_state: "deleted",
      };
    });
    const wrapper = mountView();
    await openKnowledgeTab(wrapper);
    const overlay = useOverlayStore();

    await wrapper
      .get('[data-vcp-cli-action="revoke-knowledge"]')
      .trigger("click");
    expect(overlay.confirmConfig?.message).toContain(
      "立即从未来的 vref 召回中移除",
    );
    expect(overlay.confirmConfig?.message).toContain("无法追溯收回");
    overlay.confirmConfig?.onCancel();
    await flushPromises();
    expect(revokeRequests).toHaveLength(0);
    expect(wrapper.text()).toContain("guide.md");

    await wrapper
      .get('[data-vcp-cli-action="revoke-knowledge"]')
      .trigger("click");
    overlay.confirmConfig?.onConfirm();
    await vi.waitFor(() =>
      expect(wrapper.text()).toContain("catalog write failed"),
    );
    expect(wrapper.text()).toContain("guide.md");

    await wrapper
      .get('[data-vcp-cli-action="revoke-knowledge"]')
      .trigger("click");
    overlay.confirmConfig?.onConfirm();
    await vi.waitFor(() => expect(revokeRequests).toHaveLength(2));
    await vi.waitFor(() => expect(wrapper.text()).not.toContain("guide.md"));

    expect(revokeRequests).toHaveLength(2);
    expect(revokeRequests[0].operation_id).toBe(revokeRequests[1].operation_id);
    expect(revokeRequests[0]).toMatchObject({
      source_id: "knowledge-source-1",
      expected_catalog_generation: 4,
    });
    expect(wrapper.text()).not.toContain("guide.md");
    expect(wrapper.text()).toContain("已撤权并删除本机知识对象");
    wrapper.unmount();
  });

  it("keeps one ambiguous commit owner and blocks the competing discard path", async () => {
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND, (args) => ({
      operation_id: (args?.request as { operation_id: string }).operation_id,
      status: "candidate",
      candidate: candidate(),
    }));
    const commitRequests: Array<Record<string, unknown>> = [];
    let attempt = 0;
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_COMMIT_COMMAND, (args) => {
      const request = args?.request as Record<string, unknown>;
      commitRequests.push(request);
      attempt += 1;
      if (attempt === 1) throw new Error("ambiguous commit");
      return {
        operation_id: request.operation_id,
        catalog_generation: 5,
        replayed: true,
        source: catalog().sources[0],
      };
    });
    const wrapper = mountView();
    await openKnowledgeTab(wrapper);
    await wrapper
      .get('[data-vcp-cli-action="inspect-knowledge-import"]')
      .trigger("click");
    await flushPromises();
    const overlay = useOverlayStore();

    await wrapper
      .get('[data-vcp-cli-action="commit-knowledge-import"]')
      .trigger("click");
    overlay.confirmConfig?.onConfirm();
    await vi.waitFor(() =>
      expect(wrapper.text()).toContain("ambiguous commit"),
    );
    expect(
      wrapper
        .get('[data-vcp-cli-action="discard-knowledge-import"]')
        .attributes("disabled"),
    ).toBeDefined();
    expect(
      wrapper.get('[data-vcp-cli-action="commit-knowledge-import"]').text(),
    ).toContain("重试授权");

    await wrapper
      .get('[data-vcp-cli-action="commit-knowledge-import"]')
      .trigger("click");
    overlay.confirmConfig?.onConfirm();
    await vi.waitFor(() => expect(commitRequests).toHaveLength(2));
    await vi.waitFor(() =>
      expect(
        wrapper.find('[data-vcp-cli-role="knowledge-import-review"]').exists(),
      ).toBe(false),
    );
    expect(commitRequests[0].operation_id).toBe(commitRequests[1].operation_id);
    wrapper.unmount();
  });

  it("does not commit a candidate when the view closes behind confirmation", async () => {
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND, (args) => ({
      operation_id: (args?.request as { operation_id: string }).operation_id,
      status: "candidate",
      candidate: candidate(),
    }));
    const commit = vi.fn();
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_COMMIT_COMMAND, commit);
    const wrapper = mountView();
    await openKnowledgeTab(wrapper);
    await wrapper
      .get('[data-vcp-cli-action="inspect-knowledge-import"]')
      .trigger("click");
    await flushPromises();
    await wrapper
      .get('[data-vcp-cli-action="commit-knowledge-import"]')
      .trigger("click");
    const overlay = useOverlayStore();

    await wrapper.setProps({ isOpen: false });
    overlay.confirmConfig?.onConfirm();
    await flushPromises();

    expect(commit).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("drops late catalog and inspect projections after a newer generation or closed view", async () => {
    const firstCatalog = deferred<VcpCliKnowledgeCatalog>();
    const secondCatalog = deferred<VcpCliKnowledgeCatalog>();
    let catalogRead = 0;
    mockInvoke(VCP_CLI_KNOWLEDGE_CATALOG_COMMAND, () => {
      catalogRead += 1;
      return catalogRead === 1 ? firstCatalog.promise : secondCatalog.promise;
    });
    const inspection = deferred<Record<string, unknown>>();
    mockInvoke(
      VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND,
      () => inspection.promise,
    );
    const wrapper = mountView();
    await wrapper.get('[data-vcp-cli-tab="knowledge"]').trigger("click");
    const store = useVcpCliStore();
    const newerLoad = store.loadKnowledgeCatalog();
    secondCatalog.resolve(catalog({ catalog_generation: 6 }));
    await newerLoad;
    firstCatalog.resolve(catalog({ catalog_generation: 5, sources: [] }));
    await flushPromises();
    expect(store.knowledgeCatalog?.catalog_generation).toBe(6);
    expect(store.knowledgeCatalog?.sources).toHaveLength(1);

    const inspect = store.inspectKnowledgeImport();
    await flushPromises();
    await wrapper.setProps({ isOpen: false });
    inspection.resolve({
      operation_id: store.pendingKnowledgeInspectOperationId,
      status: "candidate",
      candidate: candidate({ display_name: "late-secret.txt" }),
    });
    await inspect;
    expect(store.knowledgeMutationBusy).toBe(false);
    expect(store.knowledgeImportCandidate).toBeNull();
    expect(wrapper.text()).not.toContain("late-secret.txt");
    wrapper.unmount();
  });

  it("wires discard as a replayable owner operation rather than a local clear", async () => {
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND, (args) => ({
      operation_id: (args?.request as { operation_id: string }).operation_id,
      status: "candidate",
      candidate: candidate(),
    }));
    const discardRequests: Array<Record<string, unknown>> = [];
    let attempt = 0;
    mockInvoke(VCP_CLI_KNOWLEDGE_IMPORT_DISCARD_COMMAND, (args) => {
      const request = args?.request as Record<string, unknown>;
      discardRequests.push(request);
      attempt += 1;
      if (attempt === 1) throw new Error("ambiguous discard");
      return { operation_id: request.operation_id, replayed: true };
    });
    const wrapper = mountView();
    await openKnowledgeTab(wrapper);
    await wrapper
      .get('[data-vcp-cli-action="inspect-knowledge-import"]')
      .trigger("click");
    await flushPromises();

    await wrapper
      .get('[data-vcp-cli-action="discard-knowledge-import"]')
      .trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("notes.md");
    await wrapper
      .get('[data-vcp-cli-action="discard-knowledge-import"]')
      .trigger("click");
    await flushPromises();

    expect(discardRequests).toHaveLength(2);
    expect(discardRequests[0].operation_id).toBe(
      discardRequests[1].operation_id,
    );
    expect(discardRequests[0]).toMatchObject({
      token: "vcp-knowledge-candidate:opaque-1",
    });
    expect(
      wrapper.find('[data-vcp-cli-role="knowledge-import-review"]').exists(),
    ).toBe(false);
    wrapper.unmount();
  });
});
