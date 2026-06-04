# Breakpoint Hook - Human Tests

## Mô tả
Kiểm thử thực tế tính năng breakpoint hook: bật/tắt breakpoint, hook request/response, pause/resume, và chỉnh sửa nội dung.

## Tiền đề
- Bifrost đang chạy với `--no-system-proxy`
- WebUI mở tại `http://localhost:8800/_bifrost/`
- Có ít nhất một request HTTP đang được gửi qua proxy

## Test Cases

### TC-BP-01: Bật/tắt Breakpoint qua Toolbar
**Mục tiêu:** Kiểm tra toggle breakpoint hoạt động đúng

**Bước:**
1. Mở WebUI, vào tab Traffic
2. Quan sát Toolbar phía trên, thấy section "Breakpoint" với icon ⏸, label "Breakpoint", và Switch
3. Bật Switch Breakpoint → thấy checkbox "Req" và "Res" xuất hiện
4. Tắt Switch Breakpoint → checkbox biến mất

**Kết quả mong đợi:**
- Switch hoạt động toggle mượt
- Checkbox Req/Res chỉ hiện khi breakpoint ON
- Icon đổi màu khi ON (primary) và OFF (secondary)

**Kết quả thực tế:** (điền sau khi test)

---

### TC-BP-02: Bật Hook Request
**Mục tiêu:** Kiểm tra hook request hoạt động

**Bước:**
1. Bật Switch Breakpoint
2. Tích checkbox "Req"
3. Gửi một HTTP request qua proxy (VD: `curl -x http://localhost:8800 http://httpbin.org/get`)
4. Quan sát Traffic list → request mới xuất hiện với trạng thái "pending" (chưa có response)
5. Trong TrafficDetail, thấy biểu tượng ⏸ màu cam và nút ▶ Resume màu xanh
6. Click ▶ Resume để tiếp tục
7. Request được gửi đi và nhận response bình thường

**Kết quả mong đợi:**
- Request bị pause trước khi gửi upstream
- TrafficDetail hiện pause indicator và resume button
- Sau khi resume, request hoàn thành bình thường

**Kết quả thực tế:** (điền sau khi test)

---

### TC-BP-03: Bật Hook Response
**Mục tiêu:** Kiểm tra hook response hoạt động

**Bước:**
1. Bật Switch Breakpoint
2. Tích checkbox "Res" (bỏ tích "Req")
3. Gửi HTTP request qua proxy
4. Request được gửi đi và nhận response từ upstream
5. Response bị pause trước khi trả về client
6. Mở TrafficDetail và kiểm tra Response Body đã có đầy đủ nội dung upstream trước khi bấm Resume
7. Click ▶ Resume để trả response về client

**Kết quả mong đợi:**
- Request gửi upstream bình thường, response bị pause
- Khi đang pause, TrafficDetail đọc được full response body ngay, không cần resume mới thấy
- TrafficDetail hiện đúng pause indicator
- Sau resume, response trả về client

**Kết quả thực tế:** (điền sau khi test)

---

### TC-BP-04: Tích cả Req + Res
**Mục tiêu:** Kiểm tra hook cả request và response

**Bước:**
1. Bật Breakpoint, tích cả Req và Res
2. Gửi HTTP request
3. Request bị pause → click Resume
4. Request gửi upstream, nhận response
5. Response bị pause → click Resume
6. Response trả về client

**Kết quả mong đợi:**
- Cả 2 phase đều bị pause, mỗi lần cần click Resume riêng
- Sau 2 lần resume, request hoàn thành

**Kết quả thực tế:** (điền sau khi test)

---

### TC-BP-05: Tắt breakpoint khi đang pause
**Mục tiêu:** Kiểm tra tắt breakpoint sẽ cancel các request đang pause

**Bước:**
1. Bật Breakpoint, tích Req
2. Gửi request → bị pause
3. Tắt Switch Breakpoint
4. Các request đang pause tự động được resume/cancel

**Kết quả mong đợi:**
- Khi tắt breakpoint, tất cả pending request được giải phóng
- TrafficDetail không còn hiện pause indicator

**Kết quả thực tế:** (điền sau khi test)

---

### TC-BP-06: Regression - Chức năng bình thường khi breakpoint OFF
**Mục tiêu:** Đảm bảo khi breakpoint tắt, mọi thứ hoạt động như cũ

**Bước:**
1. Đảm bảo Breakpoint Switch đang OFF
2. Gửi nhiều HTTP request qua proxy
3. Kiểm tra traffic hiển thị bình thường, không delay
4. Kiểm tra các chức năng khác: filter, search, clear traffic

**Kết quả mong đợi:**
- Không có delay hay thay đổi hành vi khi breakpoint OFF
- Tất cả chức năng traffic hoạt động bình thường

**Kết quả thực tế:** (điền sau khi test)
