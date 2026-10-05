-- wrk script: authenticated GET, optional gzip.
-- Cookie / Accept-Encoding come from the environment so the same file
-- covers every HTTP-suite route without regenerating Lua per cell.
--
--   BENCH_COOKIE   the lane's session Cookie header value
--   BENCH_GZIP     "1" -> Accept-Encoding: gzip, else identity
init = function(_args)
  wrk.headers["Cookie"] = os.getenv("BENCH_COOKIE") or ""
  if os.getenv("BENCH_GZIP") == "1" then
    wrk.headers["Accept-Encoding"] = "gzip"
  else
    wrk.headers["Accept-Encoding"] = "identity"
  end
end
