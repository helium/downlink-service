#!/usr/bin/env bash

set -euo pipefail

VERSION=$(git describe)
SERVICE=downlink_service
CONF_PATH=./pkg/settings-template.toml

# install fpm
sudo apt update
sudo apt install --yes ruby
sudo gem install fpm -v 1.14.2 # current as of 2022-11-08

# XXX HACK fpm won't let us mark a config file unless
# it exists at the specified path
mkdir -p /opt/${SERVICE}/etc
touch /opt/${SERVICE}/etc/settings.toml

fpm -n ${SERVICE} \
    -v "${VERSION}" \
    -s dir \
    -t deb \
    --deb-systemd "/tmp/${SERVICE}.service" \
    --before-install "/tmp/${SERVICE}-preinst" \
    --after-install "/tmp/${SERVICE}-postinst" \
    --after-remove "/tmp/${SERVICE}-postrm" \
    --deb-no-default-config-files \
    --deb-systemd-enable \
    --deb-systemd-auto-start \
    --deb-systemd-restart-after-upgrade \
    --deb-user helium \
    --deb-group helium \
    --config-files /opt/${SERVICE}/etc/settings.toml \
    target/release/${SERVICE}=/opt/${SERVICE}/bin/${SERVICE} \
    $CONF_PATH=/opt/${SERVICE}/etc/settings-example.toml
