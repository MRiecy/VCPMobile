import { describe, expect, it } from "vitest";
import cliStoreSource from "@/features/cli/vcpCliStore.ts?raw";
import knowledgePanelSource from "@/features/cli/components/VcpCliKnowledgePanel.vue?raw";
import manifestViewSource from "@/features/cli/components/VcpCliManifestView.vue?raw";
import manifestGoldenSource from "../../../../src-tauri/src/vcp_modules/cli/fixtures/vcp_mobile_cli_manifest.golden.json?raw";
import knowledgeRustSource from "../../../../src-tauri/src/vcp_modules/cli/knowledge.rs?raw";
import protocolSource from "../../../../src-tauri/src/vcp_modules/cli/protocol.rs?raw";
import tauriLibSource from "../../../../src-tauri/src/lib.rs?raw";

const knowledgeCommands = [
  "get_vcp_mobile_cli_knowledge_catalog",
  "inspect_vcp_mobile_cli_knowledge_import",
  "commit_vcp_mobile_cli_knowledge_import",
  "discard_vcp_mobile_cli_knowledge_import",
  "revoke_vcp_mobile_cli_knowledge_grant",
] as const;

function rustStructFields(name: string): string[] {
  const match = knowledgeRustSource.match(
    new RegExp("pub struct " + name + "\\s*\\{([\\s\\S]*?)\\n\\}"),
  );
  expect(match, name + " Rust DTO should exist").not.toBeNull();
  return Array.from(match?.[1].matchAll(/pub\s+([a-z][a-z0-9_]*):/g) ?? []).map(
    (item) => item[1],
  );
}

function tsInterfaceFields(name: string): string[] {
  const match = cliStoreSource.match(
    new RegExp("export interface " + name + "\\s*\\{([\\s\\S]*?)\\n\\}"),
  );
  expect(match, name + " TypeScript DTO should exist").not.toBeNull();
  return Array.from(
    match?.[1].matchAll(/^\s+([a-z][a-z0-9_]*)(?:\?)?:/gm) ?? [],
  ).map((item) => item[1]);
}

describe("VCP CLI P4.4 frontend governance", () => {
  it("keeps the five knowledge commands in one CLI store owner", () => {
    for (const command of knowledgeCommands) {
      expect(cliStoreSource).toContain('"' + command + '"');
      expect(knowledgeRustSource).toContain("pub async fn " + command);
      expect(tauriLibSource).toContain(command + ",");
    }
    expect(manifestViewSource).toContain(
      'type CliTab = "run" | "skills" | "knowledge" | "manifest"',
    );
    expect(manifestViewSource).toContain("VcpCliKnowledgePanel");
    expect(manifestViewSource).toContain("store.loadKnowledgeCatalog()");
    expect(cliStoreSource).toMatch(
      /interface VcpCliKnowledgeCommitRequest[\s\S]*operation_id: string[\s\S]*token: string[\s\S]*candidate_sha256: string[\s\S]*expected_catalog_generation: number/,
    );
    expect(cliStoreSource).toMatch(
      /interface VcpCliKnowledgeRevokeRequest[\s\S]*operation_id: string[\s\S]*source_id: string[\s\S]*expected_catalog_generation: number/,
    );
    expect(cliStoreSource).toContain(
      'deletion_state: "deleted" | "pending_holds"',
    );
  });

  it("pins the exact Rust and TypeScript knowledge DTO field snapshots", () => {
    const snapshots = [
      {
        rust: "KnowledgeCatalogSnapshot",
        ts: "VcpCliKnowledgeCatalog",
        fields: [
          "schema_version",
          "catalog_generation",
          "used_bytes",
          "limit_bytes",
          "pending_used_bytes",
          "pending_limit_bytes",
          "active_source_count",
          "active_source_limit",
          "pending_candidate_count",
          "pending_candidate_limit",
          "sources",
        ],
      },
      {
        rust: "KnowledgeSourceDto",
        ts: "VcpCliKnowledgeSource",
        fields: [
          "source_id",
          "display_name",
          "mime_type",
          "size_bytes",
          "source_sha256",
          "index_status",
          "failure_code",
          "index_text_truncated",
          "chunk_count",
          "granted_at_ms",
        ],
      },
      {
        rust: "KnowledgeImportCandidate",
        ts: "VcpCliKnowledgeImportCandidate",
        fields: [
          "token",
          "candidate_sha256",
          "catalog_generation",
          "display_name",
          "mime_type",
          "size_bytes",
          "index_text_truncated",
          "chunk_count",
          "used_bytes",
          "limit_bytes",
          "pending_used_bytes",
          "pending_limit_bytes",
          "warnings",
          "replayed",
        ],
      },
      {
        rust: "InspectKnowledgeImportRequest",
        ts: "VcpCliKnowledgeInspectRequest",
        fields: ["operation_id"],
      },
      {
        rust: "CommitKnowledgeImportRequest",
        ts: "VcpCliKnowledgeCommitRequest",
        fields: [
          "operation_id",
          "token",
          "candidate_sha256",
          "expected_catalog_generation",
        ],
      },
      {
        rust: "DiscardKnowledgeImportRequest",
        ts: "VcpCliKnowledgeDiscardRequest",
        fields: ["operation_id", "token"],
      },
      {
        rust: "RevokeKnowledgeGrantRequest",
        ts: "VcpCliKnowledgeRevokeRequest",
        fields: ["operation_id", "source_id", "expected_catalog_generation"],
      },
      {
        rust: "CommitKnowledgeImportResponse",
        ts: "VcpCliKnowledgeCommitResponse",
        fields: ["operation_id", "catalog_generation", "replayed", "source"],
      },
      {
        rust: "DiscardKnowledgeImportResponse",
        ts: "VcpCliKnowledgeDiscardResponse",
        fields: ["operation_id", "replayed"],
      },
      {
        rust: "RevokeKnowledgeGrantResponse",
        ts: "VcpCliKnowledgeRevokeResponse",
        fields: [
          "operation_id",
          "catalog_generation",
          "replayed",
          "source_id",
          "deletion_state",
        ],
      },
    ] as const;

    for (const snapshot of snapshots) {
      expect(rustStructFields(snapshot.rust)).toEqual(snapshot.fields);
      expect(tsInterfaceFields(snapshot.ts)).toEqual(snapshot.fields);
    }
    expect(rustStructFields("InspectKnowledgeImportResponse")).toEqual([
      "operation_id",
      "status",
      "candidate",
    ]);
    expect(knowledgeRustSource).toContain(
      '#[serde(rename_all = "snake_case")]',
    );
    expect(knowledgeRustSource).toMatch(
      /enum KnowledgeIndexStatus[\s\S]*\bIndexing\b[\s\S]*\bReady\b[\s\S]*\bFailed\b/,
    );
    expect(cliStoreSource).toContain('"indexing" | "ready" | "failed"');
    expect(cliStoreSource).toContain('status: "cancelled"');
    expect(cliStoreSource).toContain('status: "candidate"');
  });

  it("never leaks knowledge host identity through the WebView contract", () => {
    const knowledgeDtoSource = cliStoreSource.slice(
      cliStoreSource.indexOf("export type VcpCliKnowledgeIndexStatus"),
      cliStoreSource.indexOf("interface NativePickedFile"),
    );
    const inspectBody = cliStoreSource.slice(
      cliStoreSource.indexOf("async function inspectKnowledgeImport"),
      cliStoreSource.indexOf("async function commitKnowledgeImport"),
    );
    expect(inspectBody).toContain("VCP_CLI_KNOWLEDGE_IMPORT_INSPECT_COMMAND");
    expect(inspectBody).toMatch(
      /request:\s*\{\s*operation_id: operationId,\s*\}\s+satisfies VcpCliKnowledgeInspectRequest/,
    );
    expect(inspectBody).not.toContain("VCP_CLI_NATIVE_PICK_FILE_COMMAND");
    expect(inspectBody).not.toMatch(/\b(path|uri|staging|picked)\b/i);

    const knowledgeSources = [
      knowledgeDtoSource,
      knowledgePanelSource,
      inspectBody,
    ].join("\n");
    expect(knowledgeSources).not.toMatch(
      /attachmentStore|AttachmentViewer|convertFileSrc|pick_file|\b(path|uri|staging)\b/i,
    );
    expect(knowledgePanelSource).not.toContain("v-html");
  });

  it("keeps knowledge management out of Agent actions and manifest actions", () => {
    for (const action of [
      "import_knowledge",
      "list_knowledge",
      "grant_knowledge",
      "revoke_knowledge",
      "read_knowledge",
    ]) {
      expect(cliStoreSource).not.toContain(`action: "${action}"`);
      expect(protocolSource).not.toContain(`"${action}"`);
      expect(manifestGoldenSource).not.toContain(action);
    }

    expect(protocolSource).toContain('"vref"');
    expect(protocolSource).toMatch(
      /const RUN:[\s\S]*?"vref"[\s\S]*?const LIST_SKILLS:/,
    );
    expect(protocolSource).toContain(
      "if self.vref.is_some() && !capabilities.vref",
    );
  });

  it("preserves the dense opaque UI policy without a new overlay or state store", () => {
    const sources = [knowledgePanelSource, manifestViewSource].join("\n");
    expect(knowledgePanelSource).toContain("border-l-2");
    expect(knowledgePanelSource).toContain("font-mono");
    expect(knowledgePanelSource).toContain("useOverlayStore");
    expect(knowledgePanelSource).not.toContain("defineStore");
    expect(sources).not.toContain(["backdrop", "blur"].join("-"));
    expect(sources).not.toContain("rounded-2xl");
    expect(sources).not.toContain("rounded-3xl");
    expect(sources).not.toMatch(/z-\[|\bz-[0-9]+\b/);
  });
});
