/* sphttp.c - POSIX HTTP plumbing for Tep, called from Spinel via FFI.
 *
 * The MVP stays single-threaded blocking; perf primitives (SO_REUSEPORT
 * for prefork, keep-alive friendly recv, and a "accept after fork" path)
 * are exposed so the Ruby side can do the rest. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <netdb.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <arpa/inet.h>
#include <signal.h>
#include <time.h>

#define SPHTTP_BUFSIZE   65536
#define SPHTTP_RESP_MAX  (4 * 1024 * 1024)

static char sphttp_req_buf[SPHTTP_BUFSIZE];
static int  sphttp_req_len = 0;

/* Bind & listen on 0.0.0.0:port. If `reuseport` != 0 we set
 * SO_REUSEPORT so multiple worker processes can listen on the same
 * port and the kernel will load-balance accept() across them. */
int sphttp_listen(int port, int reuseport) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;

    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
#ifdef SO_REUSEPORT
    if (reuseport) {
        setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &one, sizeof(one));
    }
#endif
    /* Disable Nagle for small response latency. */
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));

    /* Don't die on broken-pipe sends. */
    signal(SIGPIPE, SIG_IGN);

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((unsigned short)port);
    addr.sin_addr.s_addr = htonl(INADDR_ANY);

    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        close(fd);
        return -1;
    }
    if (listen(fd, 1024) < 0) {
        close(fd);
        return -1;
    }
    return fd;
}

int sphttp_accept(int sfd) {
    struct sockaddr_in caddr;
    socklen_t clen = sizeof(caddr);
    int fd;
    do {
        fd = accept(sfd, (struct sockaddr *)&caddr, &clen);
    } while (fd < 0 && errno == EINTR);
    return fd;
}

/* Read until end-of-headers ("\r\n\r\n") or the buffer fills. Subsequent
 * recv()s for the body are the caller's job (we expose a length helper).
 * Returns the parsed length (>0), 0 on clean EOF, -1 on error. */
int sphttp_read_request(int fd) {
    sphttp_req_len = 0;
    sphttp_req_buf[0] = '\0';
    while (sphttp_req_len < SPHTTP_BUFSIZE - 1) {
        ssize_t n = recv(fd, sphttp_req_buf + sphttp_req_len,
                         SPHTTP_BUFSIZE - 1 - sphttp_req_len, 0);
        if (n == 0) {
            if (sphttp_req_len == 0) return 0;
            break;
        }
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        sphttp_req_len += (int)n;
        sphttp_req_buf[sphttp_req_len] = '\0';
        if (strstr(sphttp_req_buf, "\r\n\r\n") != NULL) break;
    }
    return sphttp_req_len;
}

const char *sphttp_request_buf(void) {
    return sphttp_req_buf;
}

int sphttp_request_len(void) {
    return sphttp_req_len;
}

/* Drain the body bytes we still owe past the buffered chunk. Tep
 * computes remaining = content_length - already_in_buf; this gulps
 * those into a Ruby-visible string buffer. We round-trip via a
 * static buffer to avoid hand-rolling write_str FFI. */
static char sphttp_body_buf[SPHTTP_BUFSIZE];

const char *sphttp_drain_body(int fd, int total_len) {
    int n = total_len;
    if (n < 0) n = 0;
    if (n >= SPHTTP_BUFSIZE) n = SPHTTP_BUFSIZE - 1;
    int got = 0;
    while (got < n) {
        ssize_t r = recv(fd, sphttp_body_buf + got, n - got, 0);
        if (r <= 0) {
            if (errno == EINTR) continue;
            break;
        }
        got += (int)r;
    }
    sphttp_body_buf[got] = '\0';
    return sphttp_body_buf;
}

int sphttp_write_str(int fd, const char *s) {
    size_t len = strlen(s);
    size_t off = 0;
    while (off < len) {
        ssize_t n = send(fd, s + off, len - off, 0);
        if (n <= 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        off += (size_t)n;
    }
    return 0;
}

/* Send a file's contents straight from disk -- used for static
 * file serving. Returns -1 on open/read failure (caller falls back
 * to 404), 0 on success. */
int sphttp_sendfile(int fd, const char *path) {
    int src = open(path, O_RDONLY);
    if (src < 0) return -1;
    char buf[16384];
    for (;;) {
        ssize_t r = read(src, buf, sizeof(buf));
        if (r <= 0) break;
        ssize_t off = 0;
        while (off < r) {
            ssize_t w = send(fd, buf + off, r - off, 0);
            if (w <= 0) {
                if (errno == EINTR) continue;
                close(src);
                return -1;
            }
            off += w;
        }
    }
    close(src);
    return 0;
}

/* Returns the file size at `path`, or -1 if missing / not a regular file.
 * Used by static serving to compute Content-Length. */
int sphttp_filesize(const char *path) {
    struct stat st;
    if (stat(path, &st) < 0) return -1;
    if ((st.st_mode & S_IFMT) != S_IFREG) return -1;
    if (st.st_size > 0x7fffffff) return -1;
    return (int)st.st_size;
}

int sphttp_close(int fd) {
    return close(fd);
}

/* Chunked Transfer-Encoding frame: write `<hex-size>\r\n<bytes>\r\n`.
 * Returns 0 on success, -1 on partial write / EOF. */
int sphttp_write_chunk(int fd, const char *s) {
    size_t len = strlen(s);
    if (len == 0) return 0;
    char hdr[32];
    int n = snprintf(hdr, sizeof(hdr), "%zx\r\n", len);
    if (n <= 0) return -1;
    if (sphttp_write_str(fd, hdr) < 0) return -1;
    size_t off = 0;
    while (off < len) {
        ssize_t w = send(fd, s + off, len - off, 0);
        if (w <= 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        off += (size_t)w;
    }
    return sphttp_write_str(fd, "\r\n");
}

/* End-of-chunked-stream marker. */
int sphttp_write_chunk_end(int fd) {
    return sphttp_write_str(fd, "0\r\n\r\n");
}

/* ---------- SHA-256 + HMAC for the cookie session store ---------- */
/* Compact public-domain SHA-256 implementation, then HMAC on top. */

static const uint32_t sphttp_sha256_k[64] = {
  0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
  0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
  0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
  0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
  0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
  0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
  0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
  0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2
};

#define SPHTTP_ROTR(x,n)    (((x) >> (n)) | ((x) << (32 - (n))))
#define SPHTTP_S0(x)  (SPHTTP_ROTR(x, 2) ^ SPHTTP_ROTR(x,13) ^ SPHTTP_ROTR(x,22))
#define SPHTTP_S1(x)  (SPHTTP_ROTR(x, 6) ^ SPHTTP_ROTR(x,11) ^ SPHTTP_ROTR(x,25))
#define SPHTTP_s0(x)  (SPHTTP_ROTR(x, 7) ^ SPHTTP_ROTR(x,18) ^ ((x) >> 3))
#define SPHTTP_s1(x)  (SPHTTP_ROTR(x,17) ^ SPHTTP_ROTR(x,19) ^ ((x) >> 10))
#define SPHTTP_CH(x,y,z)  (((x) & (y)) ^ (~(x) & (z)))
#define SPHTTP_MAJ(x,y,z) (((x) & (y)) ^ ((x) & (z)) ^ ((y) & (z)))

static void sphttp_sha256_block(uint32_t H[8], const uint8_t b[64]) {
    uint32_t w[64], a, sa, sb, sc, sd, se, sf, sg, sh, t1, t2;
    int i;
    for (i = 0; i < 16; i++) {
        w[i] = ((uint32_t)b[i*4] << 24) | ((uint32_t)b[i*4+1] << 16) |
               ((uint32_t)b[i*4+2] << 8) |  (uint32_t)b[i*4+3];
    }
    for (i = 16; i < 64; i++) {
        w[i] = SPHTTP_s1(w[i-2]) + w[i-7] + SPHTTP_s0(w[i-15]) + w[i-16];
    }
    sa=H[0]; sb=H[1]; sc=H[2]; sd=H[3]; se=H[4]; sf=H[5]; sg=H[6]; sh=H[7];
    for (i = 0; i < 64; i++) {
        t1 = sh + SPHTTP_S1(se) + SPHTTP_CH(se,sf,sg) + sphttp_sha256_k[i] + w[i];
        t2 = SPHTTP_S0(sa) + SPHTTP_MAJ(sa,sb,sc);
        sh = sg; sg = sf; sf = se; se = sd + t1;
        sd = sc; sc = sb; sb = sa; sa = t1 + t2;
    }
    H[0]+=sa; H[1]+=sb; H[2]+=sc; H[3]+=sd;
    H[4]+=se; H[5]+=sf; H[6]+=sg; H[7]+=sh;
    a = a;  /* silence unused-var if compiler is pedantic */
}

static void sphttp_sha256(const uint8_t *msg, size_t len, uint8_t out[32]) {
    uint32_t H[8] = {
        0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
        0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19
    };
    uint8_t buf[64];
    size_t i, full = len & ~((size_t)63);
    for (i = 0; i < full; i += 64) sphttp_sha256_block(H, msg + i);
    size_t rem = len - full;
    for (i = 0; i < rem; i++) buf[i] = msg[full + i];
    buf[rem] = 0x80;
    if (rem >= 56) {
        for (i = rem + 1; i < 64; i++) buf[i] = 0;
        sphttp_sha256_block(H, buf);
        for (i = 0; i < 56; i++) buf[i] = 0;
    } else {
        for (i = rem + 1; i < 56; i++) buf[i] = 0;
    }
    uint64_t bits = (uint64_t)len * 8;
    for (i = 0; i < 8; i++) buf[56 + i] = (uint8_t)(bits >> (56 - 8*i));
    sphttp_sha256_block(H, buf);
    for (i = 0; i < 8; i++) {
        out[i*4]   = (uint8_t)(H[i] >> 24);
        out[i*4+1] = (uint8_t)(H[i] >> 16);
        out[i*4+2] = (uint8_t)(H[i] >> 8);
        out[i*4+3] = (uint8_t)(H[i]);
    }
}

static void sphttp_hmac_sha256(const uint8_t *key, size_t klen,
                               const uint8_t *msg, size_t mlen,
                               uint8_t out[32]) {
    uint8_t kpad[64], ipad[64], opad[64], inner[32];
    size_t i;
    if (klen > 64) {
        sphttp_sha256(key, klen, kpad);
        for (i = 32; i < 64; i++) kpad[i] = 0;
    } else {
        for (i = 0; i < klen; i++) kpad[i] = key[i];
        for (i = klen; i < 64; i++) kpad[i] = 0;
    }
    for (i = 0; i < 64; i++) {
        ipad[i] = kpad[i] ^ 0x36;
        opad[i] = kpad[i] ^ 0x5c;
    }
    /* inner = SHA256(ipad || msg) */
    /* Avoid an extra heap alloc by streaming the two segments. */
    {
        uint32_t H[8] = {
            0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
            0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19
        };
        sphttp_sha256_block(H, ipad);
        /* Now hash msg with the carry state. Total length = 64 + mlen. */
        uint8_t buf[64];
        size_t full = mlen & ~((size_t)63);
        for (i = 0; i < full; i += 64) sphttp_sha256_block(H, msg + i);
        size_t rem = mlen - full;
        for (i = 0; i < rem; i++) buf[i] = msg[full + i];
        buf[rem] = 0x80;
        if (rem >= 56) {
            for (i = rem + 1; i < 64; i++) buf[i] = 0;
            sphttp_sha256_block(H, buf);
            for (i = 0; i < 56; i++) buf[i] = 0;
        } else {
            for (i = rem + 1; i < 56; i++) buf[i] = 0;
        }
        uint64_t bits = (uint64_t)(64 + mlen) * 8;
        for (i = 0; i < 8; i++) buf[56 + i] = (uint8_t)(bits >> (56 - 8*i));
        sphttp_sha256_block(H, buf);
        for (i = 0; i < 8; i++) {
            inner[i*4]   = (uint8_t)(H[i] >> 24);
            inner[i*4+1] = (uint8_t)(H[i] >> 16);
            inner[i*4+2] = (uint8_t)(H[i] >> 8);
            inner[i*4+3] = (uint8_t)(H[i]);
        }
    }
    /* outer = SHA256(opad || inner) */
    {
        uint32_t H[8] = {
            0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
            0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19
        };
        sphttp_sha256_block(H, opad);
        uint8_t buf[64];
        for (i = 0; i < 32; i++) buf[i] = inner[i];
        buf[32] = 0x80;
        for (i = 33; i < 56; i++) buf[i] = 0;
        uint64_t bits = (uint64_t)(64 + 32) * 8;
        for (i = 0; i < 8; i++) buf[56 + i] = (uint8_t)(bits >> (56 - 8*i));
        sphttp_sha256_block(H, buf);
        for (i = 0; i < 8; i++) {
            out[i*4]   = (uint8_t)(H[i] >> 24);
            out[i*4+1] = (uint8_t)(H[i] >> 16);
            out[i*4+2] = (uint8_t)(H[i] >> 8);
            out[i*4+3] = (uint8_t)(H[i]);
        }
    }
}

/* Public FFI: HMAC-SHA256 with the result returned as a 64-char hex
 * string in a static buffer. Both key and msg are NUL-terminated
 * (Spinel doesn't pass length alongside :str) -- adequate for our
 * cookie use, where the secret has no embedded NULs and the message
 * is URL-encoded (also NUL-free). The next call clobbers the buffer. */
static char sphttp_hmac_hex_buf[65];

const char *sphttp_hmac_sha256_hex(const char *key, const char *msg) {
    uint8_t out[32];
    sphttp_hmac_sha256((const uint8_t *)key, strlen(key),
                       (const uint8_t *)msg, strlen(msg),
                       out);
    static const char H[] = "0123456789abcdef";
    int i;
    for (i = 0; i < 32; i++) {
        sphttp_hmac_hex_buf[i*2]   = H[(out[i] >> 4) & 0xf];
        sphttp_hmac_hex_buf[i*2+1] = H[out[i] & 0xf];
    }
    sphttp_hmac_hex_buf[64] = '\0';
    return sphttp_hmac_hex_buf;
}

/* base64url alphabet (RFC 4648 §5): + and / replaced by - and _.
 * No padding -- JWT and most modern callers strip '=' on emit and
 * accept it missing on decode. */
static const char B64U[64] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/* HMAC-SHA256 over (key, msg) and base64url-encode the 32-byte
 * digest into a 43-char buffer (no padding, no NUL surprises).
 * Used by JWT signing -- JWT's JOSE encoding wants the binary HMAC
 * result base64url'd directly, never hex. */
static char sphttp_hmac_b64url_buf[44];

const char *sphttp_hmac_sha256_b64url(const char *key, const char *msg) {
    uint8_t out[32];
    sphttp_hmac_sha256((const uint8_t *)key, strlen(key),
                       (const uint8_t *)msg, strlen(msg),
                       out);
    /* 32 bytes -> 43 chars (no padding). Layout: 10 full
     * 3-byte groups (30 bytes -> 40 chars) + 1 group of 2 bytes
     * (-> 3 chars). */
    int i, j = 0;
    for (i = 0; i + 3 <= 32; i += 3) {
        uint32_t v = ((uint32_t)out[i] << 16)
                   | ((uint32_t)out[i+1] << 8)
                   | (uint32_t)out[i+2];
        sphttp_hmac_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_hmac_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        sphttp_hmac_b64url_buf[j++] = B64U[(v >> 6)  & 0x3f];
        sphttp_hmac_b64url_buf[j++] = B64U[v & 0x3f];
    }
    /* Tail: 32 % 3 == 2 bytes left -> 3 chars (no padding). */
    if (i < 32) {
        uint32_t v = ((uint32_t)out[i] << 16)
                   | (i + 1 < 32 ? ((uint32_t)out[i+1] << 8) : 0);
        sphttp_hmac_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_hmac_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        if (i + 1 < 32) {
            sphttp_hmac_b64url_buf[j++] = B64U[(v >> 6) & 0x3f];
        }
    }
    sphttp_hmac_b64url_buf[j] = '\0';
    return sphttp_hmac_b64url_buf;
}

/* base64url-encode an arbitrary NUL-terminated input string. The
 * length cap below covers JWT-payload-sized inputs comfortably (a
 * 4 KiB token's payload after JSON serialisation is well under
 * 3 KiB). For larger payloads bump TEP_B64U_BUFSIZE. */
#define TEP_B64U_BUFSIZE (16 * 1024)
static char sphttp_b64url_buf[TEP_B64U_BUFSIZE];

const char *sphttp_b64url_encode(const char *src) {
    size_t n = strlen(src);
    size_t i = 0, j = 0;
    /* Worst case: 4*ceil(n/3) chars + NUL. */
    if (4 * ((n + 2) / 3) + 1 > TEP_B64U_BUFSIZE) {
        sphttp_b64url_buf[0] = '\0';
        return sphttp_b64url_buf;
    }
    while (i + 3 <= n) {
        uint32_t v = ((uint32_t)(uint8_t)src[i]   << 16)
                   | ((uint32_t)(uint8_t)src[i+1] << 8)
                   |  (uint32_t)(uint8_t)src[i+2];
        sphttp_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        sphttp_b64url_buf[j++] = B64U[(v >>  6) & 0x3f];
        sphttp_b64url_buf[j++] = B64U[ v        & 0x3f];
        i += 3;
    }
    size_t rem = n - i;
    if (rem == 1) {
        uint32_t v = (uint32_t)(uint8_t)src[i] << 16;
        sphttp_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
    } else if (rem == 2) {
        uint32_t v = ((uint32_t)(uint8_t)src[i]   << 16)
                   | ((uint32_t)(uint8_t)src[i+1] << 8);
        sphttp_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        sphttp_b64url_buf[j++] = B64U[(v >>  6) & 0x3f];
    }
    sphttp_b64url_buf[j] = '\0';
    return sphttp_b64url_buf;
}

/* base64url-decode. Returns the decoded payload as a NUL-terminated
 * C string in a static buffer. NUL bytes inside the decoded payload
 * truncate the C-string view -- not a concern for JWT (the JSON
 * payload contains no NULs by construction). */
static char sphttp_b64u_dec_buf[TEP_B64U_BUFSIZE];

static int sphttp_b64u_val(char c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return c - 'a' + 26;
    if (c >= '0' && c <= '9') return c - '0' + 52;
    if (c == '-') return 62;
    if (c == '_') return 63;
    return -1;
}

const char *sphttp_b64url_decode(const char *src) {
    size_t n = strlen(src);
    /* Strip optional padding so callers can pass either RFC-7515
     * unpadded JWT segments or the rarer padded form. */
    while (n > 0 && src[n - 1] == '=') n--;
    size_t i = 0, j = 0;
    if (n / 4 * 3 + 3 > TEP_B64U_BUFSIZE) {
        sphttp_b64u_dec_buf[0] = '\0';
        return sphttp_b64u_dec_buf;
    }
    while (i + 4 <= n) {
        int a = sphttp_b64u_val(src[i]);
        int b = sphttp_b64u_val(src[i+1]);
        int c = sphttp_b64u_val(src[i+2]);
        int d = sphttp_b64u_val(src[i+3]);
        if (a < 0 || b < 0 || c < 0 || d < 0) {
            sphttp_b64u_dec_buf[0] = '\0';
            return sphttp_b64u_dec_buf;
        }
        uint32_t v = (uint32_t)a << 18 | (uint32_t)b << 12
                   | (uint32_t)c <<  6 | (uint32_t)d;
        sphttp_b64u_dec_buf[j++] = (v >> 16) & 0xff;
        sphttp_b64u_dec_buf[j++] = (v >>  8) & 0xff;
        sphttp_b64u_dec_buf[j++] =  v        & 0xff;
        i += 4;
    }
    /* Tail: 2 chars -> 1 byte, 3 chars -> 2 bytes. */
    size_t rem = n - i;
    if (rem == 2) {
        int a = sphttp_b64u_val(src[i]);
        int b = sphttp_b64u_val(src[i+1]);
        if (a < 0 || b < 0) { sphttp_b64u_dec_buf[0] = '\0'; return sphttp_b64u_dec_buf; }
        sphttp_b64u_dec_buf[j++] = (a << 2) | (b >> 4);
    } else if (rem == 3) {
        int a = sphttp_b64u_val(src[i]);
        int b = sphttp_b64u_val(src[i+1]);
        int c = sphttp_b64u_val(src[i+2]);
        if (a < 0 || b < 0 || c < 0) { sphttp_b64u_dec_buf[0] = '\0'; return sphttp_b64u_dec_buf; }
        sphttp_b64u_dec_buf[j++] = (a << 2) | (b >> 4);
        sphttp_b64u_dec_buf[j++] = ((b & 0xf) << 4) | (c >> 2);
    }
    sphttp_b64u_dec_buf[j] = '\0';
    return sphttp_b64u_dec_buf;
}

/* PBKDF2-HMAC-SHA256 over (password, salt) with `iters` rounds.
 * Derives 32 bytes (one SHA256 block) and base64url-encodes them
 * into a 43-char unpadded string. dkLen > 32 isn't supported here
 * (matches the Tep::Password format which always derives 32). */
static char sphttp_pbkdf2_b64url_buf[44];

const char *sphttp_pbkdf2_sha256_b64url(const char *password, const char *salt, int iters) {
    if (iters < 1) iters = 1;
    size_t plen = strlen(password);
    size_t slen = strlen(salt);
    /* salt || INT(1) -- single block. */
    uint8_t salted[256];
    if (slen + 4 > sizeof(salted)) {
        sphttp_pbkdf2_b64url_buf[0] = '\0';
        return sphttp_pbkdf2_b64url_buf;
    }
    memcpy(salted, salt, slen);
    salted[slen+0] = 0;
    salted[slen+1] = 0;
    salted[slen+2] = 0;
    salted[slen+3] = 1;
    uint8_t U[32], T[32];
    sphttp_hmac_sha256((const uint8_t *)password, plen, salted, slen + 4, U);
    memcpy(T, U, 32);
    int it;
    for (it = 1; it < iters; it++) {
        sphttp_hmac_sha256((const uint8_t *)password, plen, U, 32, U);
        int b;
        for (b = 0; b < 32; b++) T[b] ^= U[b];
    }
    /* base64url-encode T (32 bytes -> 43 chars). Reuse the same
     * encoding shape as sphttp_hmac_sha256_b64url. */
    int i, j = 0;
    for (i = 0; i + 3 <= 32; i += 3) {
        uint32_t v = ((uint32_t)T[i] << 16)
                   | ((uint32_t)T[i+1] << 8)
                   | (uint32_t)T[i+2];
        sphttp_pbkdf2_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_pbkdf2_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        sphttp_pbkdf2_b64url_buf[j++] = B64U[(v >> 6)  & 0x3f];
        sphttp_pbkdf2_b64url_buf[j++] = B64U[v & 0x3f];
    }
    if (i < 32) {
        uint32_t v = ((uint32_t)T[i] << 16)
                   | (i + 1 < 32 ? ((uint32_t)T[i+1] << 8) : 0);
        sphttp_pbkdf2_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_pbkdf2_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        if (i + 1 < 32) {
            sphttp_pbkdf2_b64url_buf[j++] = B64U[(v >> 6) & 0x3f];
        }
    }
    sphttp_pbkdf2_b64url_buf[j] = '\0';
    return sphttp_pbkdf2_b64url_buf;
}

/* CSPRNG-backed random bytes, base64url-encoded. Used for
 * password salts and other unpredictable tokens. `nbytes` is
 * clamped to 64 (88 chars b64url -- enough for a 512-bit
 * random secret). */
static char sphttp_random_b64url_buf[90];

const char *sphttp_random_b64url(int nbytes) {
    if (nbytes < 1) nbytes = 16;
    if (nbytes > 64) nbytes = 64;
    uint8_t r[64];
    /* arc4random is the pragmatic CSPRNG on macOS / BSDs / glibc
     * 2.36+. For older glibc fall back to /dev/urandom. */
#if defined(__APPLE__) || defined(__FreeBSD__) || defined(__OpenBSD__) || defined(__NetBSD__)
    arc4random_buf(r, nbytes);
#else
    /* getrandom() and /dev/urandom both work; the former needs
     * <sys/random.h> on linux, the latter is universal. Keep the
     * universal path so this builds without feature-test dance. */
    FILE *f = fopen("/dev/urandom", "rb");
    if (f) {
        fread(r, 1, nbytes, f);
        fclose(f);
    } else {
        /* Last-ditch: time-mixed -- not cryptographically secure
         * but better than zeros. Callers shouldn't reach this on
         * any modern system. */
        time_t t = time(NULL);
        for (int k = 0; k < nbytes; k++) r[k] = (uint8_t)(t >> (k * 7));
    }
#endif
    int i, j = 0;
    for (i = 0; i + 3 <= nbytes; i += 3) {
        uint32_t v = ((uint32_t)r[i] << 16)
                   | ((uint32_t)r[i+1] << 8)
                   | (uint32_t)r[i+2];
        sphttp_random_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_random_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        sphttp_random_b64url_buf[j++] = B64U[(v >> 6)  & 0x3f];
        sphttp_random_b64url_buf[j++] = B64U[v & 0x3f];
    }
    int rem = nbytes - i;
    if (rem == 1) {
        uint32_t v = (uint32_t)r[i] << 16;
        sphttp_random_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_random_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
    } else if (rem == 2) {
        uint32_t v = ((uint32_t)r[i] << 16)
                   | ((uint32_t)r[i+1] << 8);
        sphttp_random_b64url_buf[j++] = B64U[(v >> 18) & 0x3f];
        sphttp_random_b64url_buf[j++] = B64U[(v >> 12) & 0x3f];
        sphttp_random_b64url_buf[j++] = B64U[(v >> 6) & 0x3f];
    }
    sphttp_random_b64url_buf[j] = '\0';
    return sphttp_random_b64url_buf;
}

/* Pre-fork support. Returns child pid in parent, 0 in child, -1 on fail. */
int sphttp_fork(void) {
    return (int)fork();
}

/* Hard exit -- bypasses spinel's Ruby-level `exit(0)` (which was
 * observed to not actually terminate child processes in some
 * codegen shapes). Used by Tep::Parallel children after they've
 * written their result file. Returns int for FFI symmetry; the
 * function actually never returns. */
int sphttp_exit(int status) {
    _exit(status);
    return 0;
}

int sphttp_getpid(void) {
    return (int)getpid();
}

/* Block until any child exits; reap it. Returns the pid that exited
 * or -1 if there are no children. */
int sphttp_wait_any(void) {
    int status = 0;
    pid_t p = wait(&status);
    return (int)p;
}

/* ---------- Non-blocking I/O + poll(2) plumbing ----------
 *
 * The scheduler parks a fiber on (fd, mode) via Sock.sphttp_poll_add;
 * tick() then calls sphttp_poll_run with a timeout and walks the
 * slots to see who got ready. Mode bits:  1=READ, 2=WRITE.
 *
 * Storage is process-static. The Ruby side owns the "reset between
 * tick rounds" discipline -- safe because the scheduler is single-
 * threaded inside one worker. */

#define SPHTTP_POLL_MAX 256
static struct pollfd sphttp_poll_set[SPHTTP_POLL_MAX];
static int           sphttp_poll_n = 0;

int sphttp_poll_reset(void) {
    sphttp_poll_n = 0;
    return 0;
}

/* Add (fd, mode_bits) to the poll set. Returns the slot index for
 * later sphttp_poll_ready(slot), or -1 if the set is full. */
int sphttp_poll_add(int fd, int mode_bits) {
    if (sphttp_poll_n >= SPHTTP_POLL_MAX) return -1;
    short ev = 0;
    if (mode_bits & 1) ev |= POLLIN;
    if (mode_bits & 2) ev |= POLLOUT;
    sphttp_poll_set[sphttp_poll_n].fd      = fd;
    sphttp_poll_set[sphttp_poll_n].events  = ev;
    sphttp_poll_set[sphttp_poll_n].revents = 0;
    return sphttp_poll_n++;
}

/* Run poll() with a millisecond timeout. -1 blocks forever, 0 is a
 * non-blocking peek. Returns the count of ready slots (>=0) or -1. */
int sphttp_poll_run(int timeout_ms) {
    int r;
    do {
        r = poll(sphttp_poll_set, sphttp_poll_n, timeout_ms);
    } while (r < 0 && errno == EINTR);
    return r;
}

/* Read the ready-mode bits for a slot. POLLHUP/POLLERR fold into the
 * READ bit so a fiber waiting on read sees the hangup and can call
 * recv() to get the 0-byte EOF / errno. */
int sphttp_poll_ready(int slot) {
    if (slot < 0 || slot >= sphttp_poll_n) return 0;
    short rev = sphttp_poll_set[slot].revents;
    int out = 0;
    if (rev & (POLLIN | POLLHUP | POLLERR)) out |= 1;
    if (rev & POLLOUT)                       out |= 2;
    return out;
}

/* Flip O_NONBLOCK on. Used by the scheduler to make handler-owned
 * sockets play nicely with poll-based parking. */
int sphttp_set_nonblock(int fd) {
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0) return -1;
    return fcntl(fd, F_SETFL, flags | O_NONBLOCK);
}

/* Outbound TCP connect. Resolves `host` via getaddrinfo (so both
 * IP literals and DNS names work). Returns the connected fd or -1.
 * Blocking connect for now -- a future variant can do non-blocking
 * connect + poll(POLLOUT) for fully-async outbound. */
int sphttp_connect(const char *host, int port) {
    struct addrinfo hints, *res = NULL;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family   = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;

    char portbuf[16];
    snprintf(portbuf, sizeof(portbuf), "%d", port);

    if (getaddrinfo(host, portbuf, &hints, &res) != 0) return -1;

    int fd = -1;
    struct addrinfo *ai;
    for (ai = res; ai != NULL; ai = ai->ai_next) {
        fd = socket(ai->ai_family, ai->ai_socktype, ai->ai_protocol);
        if (fd < 0) continue;
        if (connect(fd, ai->ai_addr, ai->ai_addrlen) == 0) break;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(res);
    if (fd < 0) return -1;

    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
    return fd;
}

/* Best-effort recv() that returns the bytes as a static buffer.
 * Pairs with sphttp_set_nonblock + sphttp_poll_run for the scheduler
 * loop. Returns "" on EAGAIN/empty so callers can branch on
 * .length == 0; "<EOF>" sentinel is the empty-string + closed fd
 * pattern (use sphttp_close + state machine on the caller side). */
static char sphttp_recv_buf[SPHTTP_BUFSIZE];
const char *sphttp_recv_some(int fd, int maxlen) {
    if (maxlen <= 0 || maxlen >= SPHTTP_BUFSIZE) maxlen = SPHTTP_BUFSIZE - 1;
    ssize_t n = recv(fd, sphttp_recv_buf, (size_t)maxlen, 0);
    if (n <= 0) {
        sphttp_recv_buf[0] = '\0';
        return sphttp_recv_buf;
    }
    sphttp_recv_buf[n] = '\0';
    return sphttp_recv_buf;
}

/* Read from `fd` until EOF (peer close) or `max_bytes`, whichever
 * comes first. Used by Tep::Http for the HTTP/1.0 + Connection:
 * close response shape. Returns the bytes in a static buffer
 * (length encoded as the C strlen, which is fine because HTTP
 * responses don't carry NUL bytes in their headers/body for the
 * formats this client targets). */
static char sphttp_recv_all_buf[SPHTTP_BUFSIZE];
const char *sphttp_recv_all(int fd, int max_bytes) {
    if (max_bytes <= 0 || max_bytes >= SPHTTP_BUFSIZE) max_bytes = SPHTTP_BUFSIZE - 1;
    int total = 0;
    while (total < max_bytes) {
        ssize_t n = recv(fd, sphttp_recv_all_buf + total, (size_t)(max_bytes - total), 0);
        if (n <= 0) break;
        total += (int)n;
    }
    sphttp_recv_all_buf[total] = '\0';
    return sphttp_recv_all_buf;
}

/* popen-based shell-out. Captures stdout (up to SPHTTP_BUFSIZE-1)
 * into a static buffer and returns it. Stderr is left to the
 * inherited fd. WARNING: cmd is passed verbatim to /bin/sh -c, so
 * NEVER interpolate untrusted input.  The Ruby side (Tep::Shell)
 * enforces this discipline at the API level. */
static char sphttp_shell_buf[SPHTTP_BUFSIZE];
const char *sphttp_shell_capture(const char *cmd, int max_bytes) {
    if (max_bytes <= 0 || max_bytes >= SPHTTP_BUFSIZE) max_bytes = SPHTTP_BUFSIZE - 1;
    sphttp_shell_buf[0] = '\0';
    FILE *fp = popen(cmd, "r");
    if (!fp) return sphttp_shell_buf;
    size_t total = 0;
    while (total < (size_t)max_bytes) {
        size_t n = fread(sphttp_shell_buf + total, 1, (size_t)max_bytes - total, fp);
        if (n == 0) break;
        total += n;
    }
    sphttp_shell_buf[total] = '\0';
    pclose(fp);
    return sphttp_shell_buf;
}

/* Atomically write `data` to `path` (truncate-and-rewrite). Used by
 * Tep::Parallel for child -> parent result IPC. Returns the number
 * of bytes written, or -1 on open/write failure. */
int sphttp_file_write(const char *path, const char *data) {
    FILE *fp = fopen(path, "w");
    if (!fp) return -1;
    size_t len = strlen(data);
    size_t written = fwrite(data, 1, len, fp);
    fclose(fp);
    if (written != len) return -1;
    return (int)written;
}

/* Read up to max_bytes from `path` into a static buffer. Useful for
 * /proc/* probes from Ruby without each call having to manage a file
 * handle. Returns "" on open failure. */
static char sphttp_file_buf[SPHTTP_BUFSIZE];
const char *sphttp_file_read(const char *path, int max_bytes) {
    if (max_bytes <= 0 || max_bytes >= SPHTTP_BUFSIZE) max_bytes = SPHTTP_BUFSIZE - 1;
    sphttp_file_buf[0] = '\0';
    FILE *fp = fopen(path, "r");
    if (!fp) return sphttp_file_buf;
    size_t total = 0;
    while (total < (size_t)max_bytes) {
        size_t n = fread(sphttp_file_buf + total, 1, (size_t)max_bytes - total, fp);
        if (n == 0) break;
        total += n;
    }
    sphttp_file_buf[total] = '\0';
    fclose(fp);
    return sphttp_file_buf;
}
