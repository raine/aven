FROM trycua/cuabot@sha256:27764b6a7867b1d7ed07975b4678e20c3005e469b0fb8178d8ee6986c14cc97b

USER root

RUN mkdir -p /var/lib/apt/lists/partial \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        fontconfig \
        fonts-jetbrains-mono \
        fonts-powerline \
        kitty \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

ARG NERD_FONTS_VERSION=v3.4.0
RUN mkdir -p /usr/local/share/fonts/JetBrainsMonoNerd \
    && curl -fsSL \
        "https://github.com/ryanoasis/nerd-fonts/releases/download/${NERD_FONTS_VERSION}/JetBrainsMono.tar.xz" \
        | tar -xJ -C /usr/local/share/fonts/JetBrainsMonoNerd \
    && fc-cache -f /usr/local/share/fonts

USER user
