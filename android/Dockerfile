# Kurogane Android build image — Rust + NDK + SDK + Gradle.
# Modeled on Ikemen GO's build/docker/android approach: one container that
# produces libkurogane.so for all ABIs and packages the APK.
#
# Build:  docker build -t kurogane-android android/
# Run:    docker run --rm -v "$PWD:/work" kurogane-android

FROM rust:1-bookworm

ENV ANDROID_HOME=/opt/android-sdk
ENV ANDROID_NDK_HOME=/opt/android-sdk/ndk/26.3.11579264
ENV JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64

RUN apt-get update && apt-get install -y --no-install-recommends \
        openjdk-17-jdk-headless unzip wget git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Android SDK: cmdline-tools → platform-tools, platform 34, build-tools, NDK.
RUN mkdir -p ${ANDROID_HOME}/cmdline-tools \
    && wget -q https://dl.google.com/android/repository/commandlinetools-linux-11076708_latest.zip -O /tmp/tools.zip \
    && unzip -q /tmp/tools.zip -d ${ANDROID_HOME}/cmdline-tools \
    && mv ${ANDROID_HOME}/cmdline-tools/cmdline-tools ${ANDROID_HOME}/cmdline-tools/latest \
    && rm /tmp/tools.zip \
    && yes | ${ANDROID_HOME}/cmdline-tools/latest/bin/sdkmanager --licenses > /dev/null \
    && ${ANDROID_HOME}/cmdline-tools/latest/bin/sdkmanager \
        "platform-tools" "platforms;android-34" "build-tools;34.0.0" \
        "ndk;26.3.11579264" > /dev/null

# Rust cross targets + cargo-ndk.
RUN rustup target add \
        aarch64-linux-android \
        armv7-linux-androideabi \
        x86_64-linux-android \
        i686-linux-android \
    && cargo install --locked cargo-ndk

WORKDIR /work
CMD ["/work/android/build.sh"]
