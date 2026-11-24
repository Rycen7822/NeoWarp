# Test NeoWarp over SSH

## Start the test container

Install Docker Desktop and start it. Make sure the `docker` CLI is on `PATH`. On macOS, you can install Docker
Desktop with:

```bash
brew install --cask docker
```

The [Build image and start container for SSH testing](../../../.warp/workflows/build_image_and_start_container_for_ssh_testing.yaml)
workflow builds from `app/tests/ssh`. Its `docker run -ditp 22:22` command can publish SSH on every host interface.
Do not run that publishing command on an untrusted network because the image uses fixed test credentials.

For local-only testing, build and run the image with a loopback-only port binding:

```bash
docker build -t ssh_test_ubuntu_latest app/tests/ssh
docker run -ditp 127.0.0.1:22:22 --name neowarp-ssh-test ssh_test_ubuntu_latest
```

Host port 22 must be available. Another container can listen on its own port 22, but it cannot publish the same host
address and port at the same time.

Connect with either supported shell account:

```bash
ssh bash@127.0.0.1
ssh zsh@127.0.0.1
```

The test accounts use `password` as their password. Use these credentials only for this isolated test container.
Do not reuse them elsewhere.

After creating the container, restart it with `docker start neowarp-ssh-test` or Docker Desktop.

## Change the SSH server configuration

The Ubuntu container reads SSH server configuration from `/etc/ssh/sshd_config`. After changing that file, restart
the SSH service:

```bash
sudo service ssh restart
```

If the container stops, restart it from Docker Desktop before reconnecting.
