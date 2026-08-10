import { describe, expect, it, vi } from "vitest";
import {
  useDistributed,
  type DistributedStatus,
} from "@/features/distributed/composables/useDistributed";
import {
  emitTauriEvent,
  invokeMock,
  listenMock,
  mockInvoke,
} from "@/tests/mocks/tauri";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function distributedStatus(
  sessionId: number,
  state: DistributedStatus["state"],
): DistributedStatus {
  return {
    state,
    connected: state === "connected",
    server_id: state === "connected" ? `server-${sessionId}` : null,
    client_id: state === "connected" ? `client-${sessionId}` : null,
    registered_tools: 0,
    last_error: null,
    session_id: sessionId,
  };
}

describe("distributed listener ownership", () => {
  it("allows listener setup to retry after a rejected registration", async () => {
    listenMock.mockRejectedValueOnce(new Error("registration failed"));
    mockInvoke("get_distributed_status", () =>
      distributedStatus(0, "disconnected"),
    );

    const distributed = useDistributed();
    await distributed.activate();
    await distributed.activate();

    expect(listenMock).toHaveBeenCalledTimes(2);
    distributed.deactivate();
  });

  it("unlistens a registration that resolves after the last consumer leaves", async () => {
    const registration = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(() => registration.promise);

    const distributed = useDistributed();
    const activating = distributed.activate();
    distributed.deactivate();
    registration.resolve(unlisten);

    await activating;
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("keeps one shared pending listener until the last consumer leaves", async () => {
    const registration = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(() => registration.promise);
    mockInvoke("get_distributed_status", () =>
      distributedStatus(0, "disconnected"),
    );

    const first = useDistributed();
    const second = useDistributed();
    const firstActivation = first.activate();
    const secondActivation = second.activate();

    expect(listenMock).toHaveBeenCalledTimes(1);
    registration.resolve(unlisten);
    await Promise.all([firstActivation, secondActivation]);

    expect(unlisten).not.toHaveBeenCalled();
    first.deactivate();
    expect(unlisten).not.toHaveBeenCalled();

    second.deactivate();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("does not let an older snapshot overwrite a newer status event", async () => {
    const snapshot = deferred<DistributedStatus>();
    mockInvoke("get_distributed_status", () => snapshot.promise);

    const distributed = useDistributed();
    const activating = distributed.activate();
    await vi.waitFor(() => {
      expect(
        invokeMock.mock.calls.some(
          ([command]) => command === "get_distributed_status",
        ),
      ).toBe(true);
    });

    emitTauriEvent("vcp-distributed-status", distributedStatus(2, "connected"));
    snapshot.resolve(distributedStatus(1, "connecting"));
    await activating;

    expect(distributed.status.value.session_id).toBe(2);
    expect(distributed.status.value.state).toBe("connected");
    distributed.deactivate();
  });
});
