#!/bin/sh
# The system libraries Bobine builds against, on an Ubuntu runner. The one list: the release and
# beta workflows call it, and `verify` in ci.yml, so that a new dependency is added here once and
# not forgotten in one of the three builds. GPUI (the window) links against the fonts, the
# keyboard maps, Wayland and X11; it opens no window on a runner, and Vulkan is only loaded at run
# time. libmpv is the player.
#
#   tools/install-build-libraries.sh
set -eu

sudo apt-get update
sudo apt-get install -y \
  libmpv-dev \
  pkg-config \
  libfontconfig-dev \
  libfreetype-dev \
  libxkbcommon-dev \
  libxkbcommon-x11-dev \
  libwayland-dev \
  libx11-xcb-dev \
  libxcb1-dev \
  libvulkan-dev
