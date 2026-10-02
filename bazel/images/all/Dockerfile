# One image for every lane the remote executors run: Ruby + Rails's needs,
# Rust for roundhouse's emitted Rust, and each target's toolchain at the
# version ci.yml pins. Built from the repository root (the scaffold Gemfile). Bazel runs actions with PATH=/bin:/usr/bin:/usr/local/bin,
# so every toolchain's commands are linked into /usr/local/bin. Each ENV here is also declared in .bazelrc's image-env.
FROM ruby:3.4

RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      sqlite3 python3 redis-server libvips42 libvips-dev libsqlite3-dev libjemalloc-dev \
      unzip xz-utils binutils gnupg2 libc6-dev libcurl4-openssl-dev libedit2 libgcc-12-dev \
      libpython3-dev libstdc++-12-dev libxml2-dev libz3-dev pkg-config tzdata zlib1g-dev libncurses-dev \
      libpcre2-dev libssl-dev libyaml-dev libgmp-dev libevent-dev ffmpeg libclang-dev clang rsync docker.io iptables >/dev/null \
 && rm -rf /var/lib/apt/lists/*

# The spinel scaffold's bundle: the ruby-family suites and archives run under BUNDLE_GEMFILE=runtime/spinel/scaffold/Gemfile.
COPY runtime/spinel/scaffold/Gemfile runtime/spinel/scaffold/Gemfile.lock /opt/scaffold/
RUN cd /opt/scaffold && bundle install --quiet

RUN gem install rails-html-sanitizer sqlite3 puma nio4r websocket-driver rack bcrypt ruby-vips \
      concurrent-ruby net-http-persistent web-push --no-document \
 && gem install activerecord -v '~> 8.1.0' --no-document

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain 1.98.1 \
 && /usr/local/cargo/bin/rustup target add wasm32-wasip1 \
 && /usr/local/cargo/bin/rustup component add rustfmt clippy \
 && ln -s /usr/local/cargo/bin/* /usr/local/bin/
RUN mkdir -p /opt/wasi-sdk && curl -fsSL https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-33/wasi-sdk-33.0-x86_64-linux.tar.gz \
      | tar -xz -C /opt/wasi-sdk --strip-components=1
ENV WASI_SDK_PATH=/opt/wasi-sdk

RUN curl -fsSL https://nodejs.org/dist/v20.20.2/node-v20.20.2-linux-x64.tar.xz | tar -xJ -C /usr/local --strip-components=1 \
 && npx -y playwright@1.56.1 install-deps chromium >/dev/null && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://go.dev/dl/go1.24.13.linux-amd64.tar.gz | tar -xz -C /usr/local \
 && ln -s /usr/local/go/bin/go /usr/local/go/bin/gofmt /usr/local/bin/

ENV UV_PYTHON_INSTALL_DIR=/opt/uv-python
RUN curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh >/dev/null \
 && uv python install 3.14 >/dev/null \
 && ln -sf "$(uv python find 3.14)" /usr/local/bin/python3.14 && ln -sf /usr/local/bin/python3.14 /usr/local/bin/python3

RUN mkdir -p /opt/java && curl -fsSL "https://api.adoptium.net/v3/binary/latest/21/ga/linux/x64/jdk/hotspot/normal/eclipse" | tar -xz -C /opt/java --strip-components=1 \
 && curl -fsSL https://services.gradle.org/distributions/gradle-9.5.1-bin.zip -o /tmp/gradle.zip && unzip -q /tmp/gradle.zip -d /opt && rm /tmp/gradle.zip \
 && ln -s /opt/java/bin/* /usr/local/bin/ && ln -s /opt/gradle-9.5.1/bin/gradle /usr/local/bin/gradle
ENV JAVA_HOME=/opt/java

RUN curl -fsSL https://download.swift.org/swift-6.1.3-release/debian12/swift-6.1.3-RELEASE/swift-6.1.3-RELEASE-debian12.tar.gz \
      | tar -xz -C /opt && ln -s /opt/swift-6.1.3-RELEASE-debian12/usr/bin/swift* /usr/local/bin/

RUN curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
 && bash /tmp/dotnet-install.sh --version 10.0.401 --install-dir /opt/dotnet >/dev/null && rm /tmp/dotnet-install.sh \
 && ln -s /opt/dotnet/dotnet /usr/local/bin/dotnet
ENV DOTNET_ROOT=/opt/dotnet DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1 DOTNET_NOLOGO=1 DOTNET_CLI_TELEMETRY_OPTOUT=1

RUN curl -fsSL https://github.com/crystal-lang/crystal/releases/download/1.21.1/crystal-1.21.1-1-linux-x86_64-bundled.tar.gz \
      | tar -xz -C /opt && ln -s /opt/crystal-1.21.1-1/bin/* /usr/local/bin/

RUN mkdir -p /opt/otp && curl -fsSL https://builds.hex.pm/builds/otp/ubuntu-22.04/OTP-27.3.4.18.tar.gz | tar -xz -C /opt/otp --strip-components=1 \
 && (cd /opt/otp && ./Install -minimal /opt/otp >/dev/null) \
 && curl -fsSL https://github.com/elixir-lang/elixir/releases/download/v1.18.5/elixir-otp-27.zip -o /tmp/elixir.zip \
 && mkdir -p /opt/elixir && unzip -q /tmp/elixir.zip -d /opt/elixir && rm /tmp/elixir.zip \
 && ln -s /opt/otp/bin/* /opt/elixir/bin/* /usr/local/bin/

RUN curl -fsSL https://repo1.maven.org/maven2/org/jruby/jruby-dist/10.0.7.0/jruby-dist-10.0.7.0-bin.tar.gz | tar -xz -C /opt \
 && ln -s /opt/jruby-10.0.7.0/bin/jruby /usr/local/bin/jruby

LABEL org.opencontainers.image.source=https://github.com/rubys/roundhouse
