// Root: declare AGP with its version, `apply false` — the :app module applies
// it versionlessly and inherits this version. AGP 8.5.2 pairs with Gradle 8.7
// (CI + ensure-gradle.sh), compileSdk 34, build-tools 34.0.0, JDK 17.
plugins {
    id("com.android.application") version "8.5.2" apply false
}
