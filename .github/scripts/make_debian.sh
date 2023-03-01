#!/usr/bin/env bash

set -euo pipefail

cd $GITHUB_WORKSPACE

if [ -z "$GITHUB_REF" ]; then
    git config --global --add safe.directory "$GITHUB_WORKSPACE"
    VERSION=$(git describe)
else
    VERSION=$(echo "$GITHUB_REF" | sed 's|refs/tags/||')
fi


write_unit_template()
{

    cat << -EOF >"/tmp/downlink_service.service"
[Unit]
Description=downlink_service
After=network.target
StartLimitInterval=60
StartLimitBurst=3

[Service]
Type=simple
ExecStart=/opt/downlink_service/bin/downlink_service -c /opt/downlink_service/etc/settings.toml
User=helium
PIDFile=/var/run/downlink_service
Restart=always
RestartSec=15
WorkingDirectory=/opt/downlink_service

### Remove default limits from a few important places:
LimitNOFILE=infinity
LimitNPROC=infinity
TasksMax=infinity

[Install]
WantedBy=multi-user.target
-EOF
}

write_prepost_template()
{
    cat << -EOF >"/tmp/downlink_service-preinst"
# add system user for file ownership and systemd user, if not exists
useradd --system --home-dir /opt/helium --create-home helium || true
-EOF

    cat << -EOF >"/tmp/downlink_service-postinst"
# add to /usr/local/bin so it appears in path
ln -s /opt/downlink_service/bin/downlink_service /usr/local/bin/downlink_service || true
-EOF

    cat << -EOF >"/tmp/downlink_service-postrm"
rm -f /usr/local/bin/downlink_service
-EOF
}

run_fpm()
{
    local VERSION=$1

    # XXX HACK fpm won't let us mark a config file unless
    # it exists at the specified path
    mkdir -p /opt/downlink_service/etc
    touch /opt/downlink_service/etc/settings.toml

    fpm -n downlink-service \
        -v "${VERSION}" \
        -s dir \
        -t deb \
        --deb-systemd "/tmp/downlink_service.service" \
        --before-install "/tmp/downlink_service-preinst" \
        --after-install "/tmp/downlink_service-postinst" \
        --after-remove "/tmp/downlink_service-postrm" \
        --deb-no-default-config-files \
        --deb-systemd-enable \
        --deb-systemd-auto-start \
        --deb-systemd-restart-after-upgrade \
        --deb-user helium \
        --deb-group helium \
        --config-files /opt/downlink_service/etc/settings.toml \
        target/release/downlink_service=/opt/downlink_service/bin/downlink_service \
        pkg/settings-template.toml=/opt/downlink_service/etc/settings-example.toml

    # copy deb to /tmp for upload later
    cp *.deb /tmp

}

# install fpm
sudo apt update
sudo apt install --yes ruby
sudo gem install fpm -v 1.15.1 # current as of 2023-02-21

write_unit_template
write_prepost_template
run_fpm $VERSION

for deb in /tmp/*.deb
do
    echo "uploading $deb"
    curl -u "${PACKAGECLOUD_API_KEY}:" \
         -F "package[distro_version_id]=210" \
         -F "package[package_file]=@$deb" \
         https://packagecloud.io/api/v1/repos/helium/packet_router/packages.json
done
