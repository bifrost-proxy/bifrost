import "./trafficStoreTestFixture";
import { useTrafficStore } from "./useTrafficStore";
import { describe, expect, it } from "vitest";
import { apiMocks, makeRecord } from "./trafficStoreTestFixture";

describe("Traffic detail selection races", () => {
  it("does not hydrate an old selected record body while the next selection loads", async () => {
    let resolveBody!: (body: string) => void;
    const body = new Promise<string>((resolve) => {
      resolveBody = resolve;
    });
    apiMocks.getRequestBody.mockReturnValue(body);
    apiMocks.getResponseBody.mockReturnValue(body);
    apiMocks.getTrafficDetail.mockResolvedValue({
      ...makeRecord("paused", "/old"),
      request_headers: null,
      response_headers: null,
      request_body: null,
      response_body: null,
      matched_rules: null,
      request_content_type: null,
    });
    useTrafficStore.setState({ selectedId: "paused", currentRecord: null });
    await useTrafficStore.getState().fetchTrafficDetail("paused");
    useTrafficStore.setState({ selectedId: "other" });
    resolveBody("stale body");
    await Promise.resolve();
    expect(useTrafficStore.getState().requestBody).toBeNull();
    expect(useTrafficStore.getState().responseBody).toBeNull();
    apiMocks.getTrafficDetail.mockReset();
    apiMocks.getRequestBody.mockReset();
    apiMocks.getResponseBody.mockReset();
  });
  it.each(["paused", "other"])(
    "ignores a scheduled refresh for an old selection before touching %s detail state",
    async (currentId) => {
      apiMocks.getTrafficDetail.mockClear();
      const current = {
        ...makeRecord(currentId, "/current"),
        request_headers: null,
        response_headers: null,
        request_body: null,
        response_body: null,
        matched_rules: null,
        request_content_type: null,
      };
      useTrafficStore.setState({
        selectedId: "other",
        currentRecord: current,
        detailLoading: false,
        detailError: "current state",
        requestBody: "current request body",
        responseBody: "current response body",
      });
      await useTrafficStore.getState().fetchTrafficDetail("paused");
      expect(apiMocks.getTrafficDetail).not.toHaveBeenCalled();
      expect(useTrafficStore.getState()).toMatchObject({
        selectedId: "other",
        currentRecord: current,
        detailLoading: false,
        detailError: "current state",
        requestBody: "current request body",
        responseBody: "current response body",
      });
    },
  );
  it.each(["other", undefined])(
    "ignores a late detail after selection changes to %s",
    async (selectedId) => {
      let resolveDetail!: (value: ReturnType<typeof makeRecord>) => void;
      apiMocks.getTrafficDetail.mockReturnValue(
        new Promise((resolve) => {
          resolveDetail = resolve;
        }),
      );
      useTrafficStore.setState({ selectedId: "paused", currentRecord: null });
      const pending = useTrafficStore.getState().fetchTrafficDetail("paused");
      useTrafficStore.setState({ selectedId });
      resolveDetail(makeRecord("paused", "/pending"));
      await pending;
      expect(useTrafficStore.getState().currentRecord).toBeNull();
      apiMocks.getTrafficDetail.mockReset();
    },
  );
  it("ignores an old request error after another selection", async () => {
    let rejectDetail!: (reason: Error) => void;
    apiMocks.getTrafficDetail.mockReturnValue(
      new Promise((_, reject) => {
        rejectDetail = reject;
      }),
    );
    useTrafficStore.setState({ selectedId: "paused", currentRecord: null });
    const pending = useTrafficStore.getState().fetchTrafficDetail("paused");
    useTrafficStore.setState({
      selectedId: "other",
      detailError: "keep current detail",
    });
    rejectDetail(new Error("stale failure"));
    await pending;
    expect(useTrafficStore.getState().detailError).toBe("keep current detail");
    apiMocks.getTrafficDetail.mockReset();
  });
});
