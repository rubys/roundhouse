-- wrk script: authenticated POST /rooms/:id/messages.
--
--   BENCH_COOKIE     session Cookie header
--   BENCH_CSRF       raw authenticity token (X-CSRF-Token header)
--   BENCH_CSRF_ENC   application/x-www-form-urlencoded token (body)
--   BENCH_GZIP       "1" -> Accept-Encoding: gzip, else identity
--
-- Body matches once-campfire-rust's loadgen: message[body], a unique
-- message[client_message_id], authenticity_token, plus Sec-Fetch-Site
-- so a header-only forgery check (the Rust port) still accepts writes.
local csrf = os.getenv("BENCH_CSRF") or ""
local csrf_enc = os.getenv("BENCH_CSRF_ENC") or ""
local n = 0
local seed = 1

init = function(_args)
  wrk.method = "POST"
  wrk.headers["Cookie"] = os.getenv("BENCH_COOKIE") or ""
  wrk.headers["Content-Type"] = "application/x-www-form-urlencoded"
  wrk.headers["Accept"] = "text/vnd.turbo-stream.html, text/html, application/xhtml+xml"
  wrk.headers["X-CSRF-Token"] = csrf
  wrk.headers["Sec-Fetch-Site"] = "same-origin"
  -- Host-compared Origin, same rule as runtime/spinel/request_forgery_protection.rb:
  -- the header's host (with port) must equal the request Host. A bare
  -- `http://127.0.0.1` against `:4300` is a 422 for every write.
  local origin = (wrk.scheme or "http") .. "://" .. (wrk.host or "127.0.0.1")
  local port = tonumber(wrk.port)
  if port and port ~= 80 and port ~= 443 then
    origin = origin .. ":" .. tostring(port)
  end
  wrk.headers["Origin"] = origin
  if os.getenv("BENCH_GZIP") == "1" then
    wrk.headers["Accept-Encoding"] = "gzip"
  else
    wrk.headers["Accept-Encoding"] = "identity"
  end
  -- wrk gives each thread its own Lua state; mix time with the table
  -- pointer so two threads starting in the same second do not collide.
  -- Strip `0x` before tonumber(..., 16): LuaJIT rejects the prefix and
  -- every thread would then share seed 0 in that second.
  local addr = tostring({}):match("0x(%x+)") or tostring({}):match("(%x+)$") or "0"
  seed = (os.time() % 100000) * 1000 + (tonumber(addr, 16) or 0) % 1000
  math.randomseed(seed)
end

request = function()
  n = n + 1
  local id = string.format("b%x-%d-%d", seed, n, math.random(1, 1000000000))
  local body = string.format(
    "message%%5Bbody%%5D=bench+write+%d&message%%5Bclient_message_id%%5D=%s&authenticity_token=%s",
    n, id, csrf_enc)
  return wrk.format(nil, nil, nil, body)
end
