# Breakpoint Hook: Request/Response Debug

## Bối cảnh
Người dùng cần khả năng pause request/response để debug: kiểm tra, sửa headers/body trước khi request được gửi đi upstream hoặc response trả về client.

## Yêu cầu

### Phạm vi
- **Tất cả HTTP request/response** khi breakpoint được bật (không filter theo rule)
- **Không hỗ trợ WebSocket** (realtime, phức tạp)
- **SSE**: buffer toàn bộ stream, chỉ cho edit khi kết nối đóng (không edit từng chunk)
- **Không sửa status code**, chỉ sửa headers + body
- **Không timeout** — pause vô thời hạn đến khi user resume

### UI
- **Toolbar**: toggle "Breakpoint" + checkbox "Hook Request" / "Hook Response" cạnh System Proxy
- **TrafficDetail**: khi record bị pause → inline edit (click trực tiếp vào cell để sửa), giữ nguyên UI hiện tại
- **Nút Resume**: 1 nút ở header TrafficDetail, gửi toàn bộ edited data
- **Traffic list**: icon ⏸ cho record đang pause
- **Khi KHÔNG pause**: mọi thứ read-only như hiện tại

## Kiến trúc

### Backend

#### BreakpointManager (`crates/bifrost-proxy/src/proxy/http/breakpoint.rs`)

```rust
// Global state, lưu trong ProxyContext hoặc Arc
struct BreakpointManager {
    enabled: AtomicBool,
    hook_request: AtomicBool,
    hook_response: AtomicBool,
    // DashMap<request_id, (Option<req_sender>, Option<res_sender>)>
    pending: DashMap<String, BreakpointHandle>,
}

struct BreakpointHandle {
    request: Option<oneshot::Sender<BreakpointEdit>>,
    response: Option<oneshot::Sender<BreakpointEdit>>,
}

struct BreakpointEdit {
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}
```

#### Pipeline Integration (`handler.rs`)

**Hook Request** — chèn sau khi đọc request body, trước khi gửi upstream (~dòng 1306):
```
nếu hook_request →
  1. push "breakpoint_request_paused" qua WebSocket (id, method, url, headers, body)
  2. await oneshot::Receiver → BreakpointEdit
  3. apply edited headers/body
  4. tiếp tục gửi upstream
```

**Hook Response** — chèn sau khi đọc response body, trước khi trả client (~dòng 2323):
```
nếu hook_response →
  1. cập nhật traffic record với full response body hiện tại
  2. push "breakpoint_response_paused" qua WebSocket (id, status, headers, body)
  3. await oneshot::Receiver → BreakpointEdit
  4. apply edited headers/body và tiếp tục trả về client
```

#### API Routes (AdminRouter)

| Method | Path | Mô tả |
|--------|------|-------|
| `GET` | `/api/breakpoint/settings` | Lấy state |
| `POST` | `/api/breakpoint/settings` | Cập nhật {enabled, hook_request, hook_response} |
| `POST` | `/api/breakpoint/resume` | Resume + edited data |

#### Push Messages (WebSocket)

| Type | Direction | Data |
|------|-----------|------|
| `breakpoint_request_paused` | Server → Client | `{request_id, method, url, headers, body}` |
| `breakpoint_response_paused` | Server → Client | `{request_id, status, headers, body}` |
| `breakpoint_resumed` | Server → Client | `{request_id}` |
| `breakpoint_settings_updated` | Server → Client | `{enabled, hook_request, hook_response}` |

### Frontend

#### Toolbar (`Toolbar/index.tsx`)

Thêm props vào ToolbarProps và render tại RIGHT section:
```
Breakpoint [Switch]
  → khi ON: ☐ Hook Request  ☐ Hook Response
System Proxy [Switch]
```

#### useBreakpointStore (`web/src/stores/useBreakpointStore.ts`)

```typescript
interface BreakpointState {
  enabled: boolean;
  hookRequest: boolean;
  hookResponse: boolean;
  pausedRequests: Map<string, PausedData>;
  pausedResponses: Map<string, PausedData>;
}

interface PausedData {
  requestId: string;
  method?: string;
  url?: string;
  status?: number;
  headers: [string, string][];
  body: string | null;
}
```

#### Inline Edit (TrafficDetail)

**Nguyên tắc**: không thay đổi UI, không thêm nút "Edit"/"Apply". Click trực tiếp vào cell để edit.

- **Header pane**: khi record đang pause → cell "Key" và "Value" trở thành `<Input>` editable inline
- **Body pane**: khi record đang pause → body content chuyển thành textarea editable cùng vị trí
- **Resume**: 1 nút ▶ ở header TrafficDetail, gửi edited data

#### PushService (`pushService.ts`)

Thêm handlers cho 4 message types mới, route đến useBreakpointStore.

### SSE Đặc biệt

- Khi hook_response bật và response là SSE → buffer toàn bộ events thay vì stream
- Toàn bộ SSE body phải được ghi vào traffic trước khi trạng thái pause được gửi lên UI
- Khi SSE stream đóng → gửi `breakpoint_response_paused` với toàn bộ text đã tích lũy
- User edit → resume → gửi edited text dạng text/event-stream

## Files cần thay đổi/tạo mới

| File | Action |
|------|--------|
| `crates/bifrost-proxy/src/proxy/http/breakpoint.rs` | **Mới** - BreakpointManager |
| `crates/bifrost-proxy/src/proxy/http/handler.rs` | **Sửa** - Tích hợp hook vào pipeline |
| `crates/bifrost-proxy/src/proxy/http/mod.rs` | **Sửa** - Thêm module breakpoint |
| `crates/bifrost-admin/src/` | **Sửa** - Thêm API routes |
| `crates/bifrost-admin/src/push.rs` | **Sửa** - Thêm message types |
| `web/src/stores/useBreakpointStore.ts` | **Mới** - Breakpoint state |
| `web/src/components/Toolbar/index.tsx` | **Sửa** - Breakpoint controls |
| `web/src/components/TrafficDetail/panes/Header/index.tsx` | **Sửa** - Inline edit |
| `web/src/components/TrafficDetail/panes/Body/index.tsx` | **Sửa** - Inline edit |
| `web/src/components/TrafficDetail/index.tsx` | **Sửa** - Resume button |
| `web/src/services/pushService.ts` | **Sửa** - Handle messages |

## Kiểm thử

### Unit test
- BreakpointManager: enable/disable, hook_request, hook_response, pause/resume cycle
- Handler: verify pause at correct pipeline points

### E2E
- Bật breakpoint, gửi request, verify pause, edit headers/body, resume, verify edited data đến upstream
- Tương tự cho response hook

### Human tests
- `human_tests/breakpoint-hook.md` - Test UI toggle, inline edit, resume flow
