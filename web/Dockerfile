# syntax=docker/dockerfile:1
# Static image for the build-free web client: Caddy serves the same files a
# local `python3 -m http.server` does. No Node/build stage — there is nothing
# to compile or bundle.
#
# Base image pinned by digest; refresh with:
#   docker buildx imagetools inspect caddy:2.11-alpine

FROM caddy:2.11-alpine@sha256:78bd35025c7ba85961dd149ae670315129954e77abe2383ac47c89106b35ade9

# Unprivileged runtime user. The upstream image grants cap_net_bind_service to
# the caddy binary, so :80 works without root; /data and /config stay writable
# for Caddy's config autosave.
RUN addgroup -S -g 10001 app \
    && adduser -S -D -H -u 10001 -G app -s /sbin/nologin app \
    && chown -R app:app /data /config /etc/caddy

COPY --chown=app:app Caddyfile /etc/caddy/Caddyfile
COPY --chown=app:app index.html styles.css /usr/share/caddy/
COPY --chown=app:app js /usr/share/caddy/js

USER app
EXPOSE 80

HEALTHCHECK --interval=30s --timeout=3s --start-period=3s --retries=3 \
    CMD wget -q -O /dev/null http://127.0.0.1/healthz || exit 1
