FROM rust:1-bookworm AS builder
WORKDIR /app
RUN rustup target add wasm32-unknown-unknown
# Install a prebuilt trunk release instead of `cargo install trunk`: installing
# from source re-resolves trunk's dependency graph against latest crates.io
# versions (ignoring trunk's own lockfile) and can pull in incompatible
# versions (e.g. lightningcss/cssparser version conflicts).
RUN curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz \
    | tar -xz -C /usr/local/bin trunk
COPY . .
RUN trunk build --release

FROM nginx:alpine AS runner
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
ENV PORT=8080
EXPOSE 8080
CMD ["/bin/sh", "-c", "envsubst '$PORT' < /etc/nginx/templates/default.conf.template > /etc/nginx/conf.d/default.conf && nginx -g 'daemon off;'"]
