# Optional Linux Development Container

This Dockerfile provides an optional Linux development environment for NeoWarp. It is separate from CI; use [CONTRIBUTING.md](../../CONTRIBUTING.md) for current prerequisites, environment variables, and build commands.

This container is based on Debian Sid, Debian's unstable branch.  It ensures that you are running the latest versions of things like `mesa` (an open-source 3D graphics library, providing implementations of OpenGL and Vulkan).

## Prerequisites

The commands below assume a macOS host with:

* Docker (e.g.: Docker Desktop)
* XQuartz (download [here](https://www.xquartz.org/))
  * You'll want to enable iGLX (indirect GL extensions) for proper rendering by running `defaults write org.xquartz.X11 enable_iglx -bool true`; you can do this before you install XQuartz.
  * After installing XQuartz, run it, and enable "Allow connections from network clients" in the Security tab in its settings.  You'll need to quit and relaunch XQuartz after making this change.

## Setup

You'll run all of these commands from the repository's root directory.

First, build the image with the repository root as its build context. The Dockerfile copies files from `script/` and `rust-toolchain.toml`:

```
IMAGE_NAME="neowarp-linux-dev"
docker build -f docker/linux-dev/Dockerfile -t "$IMAGE_NAME" .
```

Next, run the container:

```
# Run from the NeoWarp repository root.
LOCAL_PATH="$PWD"

# Expose SSH on localhost:2222 and mount this checkout as /src.
docker run -d -p 127.0.0.1:2222:22/tcp -v "$LOCAL_PATH:/src" "$IMAGE_NAME"
```

The container does not need host credentials to use the mounted checkout. If your development task needs them, explicitly add the appropriate mount, such as `-v "$HOME/.ssh:/home/dev/.ssh:ro"` or `-v "$HOME/.config/gcloud:/home/dev/.config/gcloud"`. Use your own authorized environment; these mounts do not grant access to upstream Warp infrastructure.

## Usage

Every time you start XQuartz, you'll need to run this once in order for programs running in the container to connect to it:

```
xhost +localhost
```

SSH into the container (the local dev account password is `password`):

```
ssh -p 2222 dev@localhost
cd /src
```

Inside `/src`, configure the macOS/Linux `.work/` environment from [CONTRIBUTING.md](../../CONTRIBUTING.md) before setup or builds, then use `./script/run`. Keep compilation to two jobs, tests to one thread, and one Cargo command at a time. Diagnose build failures from their output instead of repeatedly rerunning them.

`protobuf-compiler` remains necessary for the retained `remote_server` protocol. The installed `gcloud` CLI supports external Google Cloud/IAP SSH usage. Some historical SSH integration fixtures still hard-code former Warp GCP VMs; the current CI filter marks those fixtures unverified. This container and credential mounts do not make those targets available. Ordinary SSH and local daemon tests remain relevant.
