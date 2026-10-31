Sets which BuildKit builder to use, through `$BUILDX_BUILDER`.

See <https://docs.docker.com/build/building/variables/#buildx_builder>

A builder is what actually runs BuildKit, and so what runs each crate's compilation. `cargo green` either manages one for you, or uses the one you pick:

| `$BUILDX_BUILDER` | Builder used | Managed by `cargo green` |
|---|---|---|
| unset, or `"supergreen"` | the *managed* builder (named after where it runs, see below) | yes |
| `""` (empty) | none picked: `docker build`'s, ie. the daemon's own `default` builder (or, after `docker buildx install`, buildx's current one, see `docker buildx use`) | no |
| any other name | that existing builder, as is (an error if it does not exist) | no |

**The managed builder**

`cargo green` takes care of this builder so that you don't have to:
* it is created when missing, with the [`docker-container` driver](https://docs.docker.com/build/builders/drivers/docker-container/) running `$CARGOGREEN_BUILDER_IMAGE` (which defaults to the latest `moby/buildkit` image)
* it is re-created when it does not run `$CARGOGREEN_BUILDER_IMAGE`, when it runs a BuildKit older than the latest release `cargo green` knows of, or when it points at another endpoint than its daemon's (e.g. not at the connection pool, see below). Re-creating first tries to keep the builder's state (its build cache), with `docker buildx rm --keep-state`.

Where it runs depends on which daemon is configured, checked in this order:
1. [`$BUILDKIT_HOST`](https://docs.docker.com/build/building/variables/#buildkit_host) (e.g. `ssh://host`, `tcp://host:1234`, `unix:///run/buildkit/buildkitd.sock`): builds go straight to that BuildKit daemon, through a [`remote` driver](https://docs.docker.com/build/builders/drivers/remote/) builder named `"supergreen-buildkit-<address>"`. No container gets created there, so `$CARGOGREEN_BUILDER_IMAGE` cannot apply, and re-creating that builder does not upgrade the daemon.
1. [`$DOCKER_HOST`](https://docs.docker.com/reference/cli/docker/#environment-variables) (e.g. `ssh://host`, `tcp://host:2376`, `unix:///run/user/1000/docker.sock`): the builder's container runs on that docker daemon, and is named after its address (e.g. `"supergreen-<host>"`). The default `unix:///var/run/docker.sock` counts as local (the builder is then just `"supergreen"`).
1. A [docker context](https://docs.docker.com/engine/manage-resources/contexts/), from [`$DOCKER_CONTEXT`](https://docs.docker.com/reference/cli/docker/#environment-variables) or `docker context use`, whose daemon is on an `ssh://` or `tcp://` host: this is the same as setting `$DOCKER_HOST` to that address (along with the context's TLS settings).
1. Otherwise: the builder runs on the local docker daemon and is named `"supergreen"`.

Each daemon thus gets a builder of its own, and switching between daemons does not re-create builders back and forth.

**Remote daemons and the connection pool**

Builds talking to an `ssh://` or `tcp://` daemon open many connections: at least one per crate. With the `docker` runner, `cargo green` pools these so the whole build only pays for a few handshakes:
* `ssh://` uses your `ssh` command and so your `~/.ssh/config` (aliases, `ProxyJump`, `IdentityFile`, ...). Use an `ssh-agent` (or a key without a passphrase): a few connections get opened, and each would prompt otherwise.
* `$DOCKER_HOST=tcp://…` reads [`$DOCKER_TLS_VERIFY`](https://docs.docker.com/reference/cli/docker/#environment-variables) and [`$DOCKER_CERT_PATH`](https://docs.docker.com/reference/cli/docker/#environment-variables) the way `docker` does ([`$DOCKER_TLS`](https://docs.docker.com/reference/cli/docker/#environment-variables) alone, ie. without verification, is unsupported). See [protecting the daemon socket with TLS](https://docs.docker.com/engine/security/protect-access/#use-tls-https-to-protect-the-docker-daemon-socket). `$BUILDKIT_HOST=tcp://…` reads [`$CARGOGREEN_BUILDKIT_TLSDIR`](#cargogreen_buildkit_tlsdir).
* The pool is a local socket that only exists while `cargo green` runs. The managed builder points at it, so outside of `cargo green`, `docker buildx ls` shows that builder as erroring: this is expected.
* Without a remote daemon (no `$DOCKER_HOST`, `$BUILDKIT_HOST` nor remote docker context), there is no pool: builds are local.

Remote podman ([`$CONTAINER_HOST`](https://docs.podman.io/en/latest/markdown/podman.1.html#environment-variables), [`$CONTAINER_CONNECTION`](https://docs.podman.io/en/latest/markdown/podman.1.html#environment-variables)) is not supported yet: with the `podman` runner, builds run locally.

**Picking your own builder**

Set `$BUILDX_BUILDER` to the name of a builder you created to have builds run there. `cargo green` then never creates, re-creates nor upgrades it:
* when it does not run `$CARGOGREEN_BUILDER_IMAGE`, that is an error
* when its BuildKit is outdated, a warning shows how to re-create it yourself

For instance, to build on a remote docker daemon with a builder of your own:
```shell
# once: create the builder on that daemon, and start it
docker buildx create --name mine --driver docker-container --bootstrap ssh://me@host

# then, each time:
DOCKER_HOST=ssh://me@host BUILDX_BUILDER=mine cargo green build
```
* Without `--driver`, `docker buildx create … ssh://host` makes a [`remote` driver](https://docs.docker.com/build/builders/drivers/remote/) builder: it expects a bare BuildKit daemon on `host`, and builds then fail with `context deadline exceeded` when there is none.
* Without `--bootstrap`, a builder only gets its container (and its BuildKit version) once started: until then, `cargo green` cannot inspect it and its BuildKit shows as outdated. `docker buildx inspect --bootstrap mine` starts it.
* Nothing gets (re-)created on the remote daemon for you, as noted above.

Remote builds work this way, but not as the managed solution does:
* buildx dials a builder at the address recorded when it was created, ignoring `$DOCKER_HOST`: your builder runs builds on the daemon it was created on, without the connection pool (each build opens its own connections).
* Still, point `$DOCKER_HOST` (or the context) at that same daemon: `cargo green` also uses it to inspect your builder.

`$BUILDKIT_HOST` only ever selects the managed builder: buildx does not read `$BUILDKIT_HOST`, so setting it along with a non-empty `$BUILDX_BUILDER` (even `"supergreen"`) is an error.

**Other unsupported combinations**

These are errors:
* `$CARGOGREEN_BUILDER_IMAGE` with `$BUILDX_BUILDER=""` (no builder to run that image)
* both `$BUILDKIT_HOST` and `$DOCKER_HOST` (a `remote` builder does not use `$DOCKER_HOST`)

See also
* `$DOCKER_HOST`, `$DOCKER_CONTEXT`, `$DOCKER_CONFIG`, `$DOCKER_TLS`, `$DOCKER_TLS_VERIFY`, `$DOCKER_CERT_PATH`: <https://docs.docker.com/reference/cli/docker/#environment-variables>
* Docker over TLS (`$DOCKER_TLS_VERIFY`, `$DOCKER_CERT_PATH`): <https://docs.docker.com/engine/security/protect-access/#use-tls-https-to-protect-the-docker-daemon-socket>
* Docker contexts: <https://docs.docker.com/engine/manage-resources/contexts/>
* `$BUILDKIT_HOST`: <https://docs.docker.com/build/building/variables/#buildkit_host>
* `$CONTAINER_HOST`, `$CONTAINER_CONNECTION`: <https://docs.podman.io/en/latest/markdown/podman.1.html#environment-variables>

