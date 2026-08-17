# prio e2e test image
# Installs: Rust, Node 20, git, gh (GitHub CLI)
# The prio source is bind-mounted at /prio at runtime; binary is built on container start.

FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive

# ── System packages ───────────────────────────────────────────────────────────
# Includes Tauri 2 Linux build prerequisites (webkit2gtk, dbus, gtk3, etc.)
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    ca-certificates \
    git \
    build-essential \
    pkg-config \
    libssl-dev \
    xz-utils \
    gnupg \
    lsb-release \
    # Tauri / WebKitGTK dependencies
    libdbus-1-dev \
    libgtk-3-dev \
    libsoup-3.0-dev \
    libjavascriptcoregtk-4.1-dev \
    libwebkit2gtk-4.1-dev \
    librsvg2-dev \
    file \
    # Allow popen2/spawn of subprocesses in test
    procps \
    && rm -rf /var/lib/apt/lists/*

# ── GitHub CLI ────────────────────────────────────────────────────────────────
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] \
        https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y gh \
    && rm -rf /var/lib/apt/lists/*

# ── Node.js 20 ────────────────────────────────────────────────────────────────
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# ── Rust ─────────────────────────────────────────────────────────────────────
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
    && chmod -R a+w /usr/local/cargo /usr/local/rustup

# ── Git identity (used for all test commits) ──────────────────────────────────
ENV GIT_AUTHOR_NAME="prio-e2e" \
    GIT_AUTHOR_EMAIL="prio-e2e@test.local" \
    GIT_COMMITTER_NAME="prio-e2e" \
    GIT_COMMITTER_EMAIL="prio-e2e@test.local"

# Configure git globally
RUN git config --global user.name "prio-e2e" \
    && git config --global user.email "prio-e2e@test.local" \
    && git config --global init.defaultBranch main \
    && git config --global advice.detachedHead false

WORKDIR /prio

# The prio source (bind-mounted at /prio) is built at container start.
# This CMD is overridden by run.mjs which first runs cargo build then node e2e.mjs.
CMD ["bash"]
