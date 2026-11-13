#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: ipod-connection-utility-install-dependencies [options]

Check or install host prerequisites for Apple Music backend setup.

Options:
  --install          Install missing packages with the detected package manager
  --yes              Install without asking (implies --install)
  --dry-run          Print the host and package plan without making changes
  --basic            Python/venv and ffmpeg/ffprobe only; no wrapper tools
  --source           Also ensure Rust 1.89+ for installing from source
  --source-only      Check/install only the Rust source-build toolchain
  --start-docker     Start Docker Desktop or the local Docker service
  -h, --help         Show this help

Optional explicit host inputs (primarily for isolated tests):
  IPOD_OS_RELEASE_FILE     Linux os-release data file (ID and VERSION_ID only)
  IPOD_DEPENDENCY_PATH     Exact tool PATH; disables external Homebrew discovery
  IPOD_DOCKER_DESKTOP_APP  Docker.app directory instead of standard Mac locations
EOF
}

die() { echo "install-dependencies: $*" >&2; exit 1; }
install_packages=0 assume_yes=0 dry_run=0 source_build=0 backend_deps=1 basic_deps=0 start_docker=0
docker_socket_path="${IPOD_DOCKER_SOCKET_OVERRIDE:-/var/run/docker.sock}"
while (($#)); do
    case "$1" in
        --basic) basic_deps=1; backend_deps=0; shift ;;
        --install) install_packages=1; shift ;;
        --yes) assume_yes=1; install_packages=1; shift ;;
        --dry-run) dry_run=1; shift ;;
        --source) source_build=1; shift ;;
        --source-only) source_build=1; backend_deps=0; shift ;;
        --start-docker) start_docker=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

if ((basic_deps && start_docker)); then die '--basic cannot start Docker'; fi

# Explicit tool path is useful for hermetic tests and controlled deployments.
# Never augment it with tools discovered outside the requested path.
if [[ ${IPOD_DEPENDENCY_PATH+x} ]]; then
    [[ -n "$IPOD_DEPENDENCY_PATH" ]] || die 'IPOD_DEPENDENCY_PATH must not be empty'
    PATH=$IPOD_DEPENDENCY_PATH
    export PATH
fi
os_name=$(uname -s)
host_arch=$(uname -m)
if [[ "$os_name" == Darwin && ! ${IPOD_DEPENDENCY_PATH+x} ]] && ! command -v brew >/dev/null 2>&1; then
    for brew_path in /opt/homebrew/bin/brew /usr/local/bin/brew; do
        if [[ -x "$brew_path" ]]; then
            PATH="${brew_path%/*}:$PATH"
            export PATH
            break
        fi
    done
fi
os_id=""
os_version=""
os_release_file=${IPOD_OS_RELEASE_FILE-/etc/os-release}
if [[ "$os_name" == Linux ]]; then
    if [[ ${IPOD_OS_RELEASE_FILE+x} && ( ! -f "$os_release_file" || ! -r "$os_release_file" ) ]]; then
        die 'IPOD_OS_RELEASE_FILE must name a readable regular file'
    fi
    if [[ -r "$os_release_file" ]]; then
        # Read only the two literal fields we need; never execute fixture or
        # host shell code. Standard os-release IDs/versions need no expansion.
        while IFS='=' read -r key value || [[ -n "$key" ]]; do
            case "$key" in ID|VERSION_ID) ;; *) continue ;; esac
            case "$value" in
                \"*\") value=${value#\"}; value=${value%\"} ;;
                \'*\') value=${value#\'}; value=${value%\'} ;;
            esac
            case "$key" in ID) os_id=$value ;; VERSION_ID) os_version=$value ;; esac
        done < "$os_release_file"
    fi
fi

missing=""
rust_missing=0
missing_add() {
    if [[ -z "$missing" ]]; then missing=$1; else missing="$missing, $1"; fi
}
rust_is_sufficient() {
    command -v cargo >/dev/null 2>&1 || return 1
    local version major minor rest
    version=$(cargo --version 2>/dev/null | sed -n 's/^cargo \([0-9][0-9.]*\).*/\1/p')
    [[ -n "$version" ]] || return 1
    major=${version%%.*}; rest=${version#*.}; minor=${rest%%.*}
    [[ "$major" =~ ^[0-9]+$ && "$minor" =~ ^[0-9]+$ ]] || return 1
    ((major > 1 || (major == 1 && minor >= 89)))
}
c_compiler_available() {
    local compiler
    if [[ -n "${CC:-}" ]]; then compiler=${CC%% *}; else compiler=cc; fi
    if command -v "$compiler" >/dev/null 2>&1 && "$compiler" --version >/dev/null 2>&1; then
        return 0
    fi
    [[ -n "${CC:-}" ]] || { command -v gcc >/dev/null 2>&1 && gcc --version >/dev/null 2>&1; }
}

check_dependencies() {
    missing=""
    rust_missing=0
    if ((backend_deps)); then
        command -v docker >/dev/null 2>&1 || missing_add 'Docker Engine/Desktop'
        if command -v docker >/dev/null 2>&1; then
            if ((dry_run)); then
                echo 'Docker Compose/Buildx checks: deferred (dry run does not call Docker)'
            else
                docker compose version >/dev/null 2>&1 || missing_add 'Docker Compose v2'
                docker buildx version >/dev/null 2>&1 || missing_add 'Docker Buildx plugin'
            fi
        fi
        command -v git >/dev/null 2>&1 || missing_add Git
        command -v jq >/dev/null 2>&1 || missing_add jq
        command -v unzip >/dev/null 2>&1 || missing_add unzip
        command -v file >/dev/null 2>&1 || missing_add file
    fi
    if ((backend_deps || basic_deps)); then
        command -v python3 >/dev/null 2>&1 || missing_add 'Python 3.10+'
        if command -v python3 >/dev/null 2>&1 && \
            ! python3 -c 'import sys; raise SystemExit(sys.version_info < (3, 10))' >/dev/null 2>&1; then
            missing_add 'Python 3.10+ (found an older Python)'
        fi
        if ((basic_deps)) && { ! command -v python3 >/dev/null 2>&1 || ! python3 -m venv --help >/dev/null 2>&1; }; then
            missing_add 'Python venv support'
        fi
        command -v ffmpeg >/dev/null 2>&1 || missing_add ffmpeg
        command -v ffprobe >/dev/null 2>&1 || missing_add ffprobe
    fi
    if ((source_build)) && ! rust_is_sufficient; then
        rust_missing=1
        missing_add 'Rust/Cargo 1.89+'
        if ! command -v rustup >/dev/null 2>&1 && ! command -v curl >/dev/null 2>&1; then
            missing_add 'curl (needed to install Rustup)'
        fi
    fi
    if ((source_build)) && ! c_compiler_available; then
        missing_add 'C compiler/linker'
    fi
}
add_source_installer_packages() {
    if ((source_build && rust_missing)) && ! command -v curl >/dev/null 2>&1; then
        case "$package_manager" in
            pacman|apt|dnf) packages+=(curl) ;;
            brew) packages+=(curl) ;;
        esac
    fi
    if ((source_build)) && ! c_compiler_available; then
        case "$package_manager" in
            pacman) packages+=(base-devel) ;;
            apt) packages+=(build-essential) ;;
            dnf) packages+=(gcc make) ;;
        esac
    fi
}
link_homebrew_docker_plugins() {
    local brew_prefix plugin source destination
    brew_prefix=$(brew --prefix)
    for plugin in docker-compose docker-buildx; do
        source="$brew_prefix/lib/docker/cli-plugins/$plugin"
        destination="${HOME:?HOME is required}/.docker/cli-plugins/$plugin"
        if [[ -x "$source" && ! -e "$destination" && ! -L "$destination" ]]; then
            mkdir -p "${destination%/*}"
            ln -s "$source" "$destination"
        fi
    done
}

package_manager=""
packages=()
brew_cask=0
find_package_manager() {
    case "$os_name:$os_id" in
        Darwin:*)
            if command -v brew >/dev/null 2>&1; then package_manager=brew; else package_manager='missing-brew'; fi
            if ((backend_deps)); then
                packages=(git jq unzip python ffmpeg)
                if [[ ${IPOD_DOCKER_DESKTOP_APP+x} ]]; then
                    [[ -n "$IPOD_DOCKER_DESKTOP_APP" ]] || die 'IPOD_DOCKER_DESKTOP_APP must not be empty'
                    [[ -d "$IPOD_DOCKER_DESKTOP_APP" ]] || brew_cask=1
                elif [[ ! -d /Applications/Docker.app && ! -d "${HOME:-}/Applications/Docker.app" ]]; then
                    brew_cask=1
                fi
            fi
            ;;
        Linux:arch|Linux:alarm|Linux:archarm)
            command -v pacman >/dev/null 2>&1 || die 'pacman was not found'
            package_manager=pacman
            if ((backend_deps)); then
                packages=(docker docker-compose docker-buildx git jq unzip file python ffmpeg)
            fi
            ;;
        Linux:debian|Linux:ubuntu|Linux:linuxmint|Linux:raspbian)
            command -v apt-get >/dev/null 2>&1 || die 'apt-get was not found'
            package_manager=apt
            if ((backend_deps)); then
                packages=(docker.io git jq unzip file python3 python3-venv ffmpeg)
                if [[ "$os_id" == ubuntu ]]; then
                    packages+=(docker-compose-v2 docker-buildx)
                elif [[ "$os_id" == debian && "${os_version%%.*}" =~ ^[0-9]+$ ]] &&
                    (( ${os_version%%.*} >= 13 )); then
                    packages+=(docker-compose docker-buildx)
                fi
            fi
            ;;
        Linux:fedora)
            command -v dnf >/dev/null 2>&1 || die 'dnf was not found'
            package_manager=dnf
            if ((backend_deps)); then
                packages=(moby-engine docker-compose docker-buildx git jq unzip file python3 ffmpeg-free)
            fi
            ;;
        *) die "unsupported host $os_name/$host_arch (supported: Arch/ALARM, Debian/Ubuntu, Fedora, macOS)" ;;
    esac
    if ((basic_deps)); then
        case "$package_manager" in
            pacman|brew|missing-brew) packages=(python ffmpeg) ;;
            apt) packages=(python3 python3-venv ffmpeg) ;;
            dnf) packages=(python3 ffmpeg-free) ;;
        esac
    fi
}
remove_package() {
    local item="$1" current
    local remaining=()
    for current in ${packages[@]+"${packages[@]}"}; do
        [[ "$current" == "$item" ]] || remaining+=("$current")
    done
    packages=("${remaining[@]}")
}
replace_package() {
    local old="$1" new="$2" current
    local changed=()
    for current in ${packages[@]+"${packages[@]}"}; do
        if [[ "$current" == "$old" ]]; then changed+=("$new"); else changed+=("$current"); fi
    done
    packages=("${changed[@]}")
}
package_is_needed() {
    case "$1" in
        docker|docker.io|moby-engine) ! command -v docker >/dev/null 2>&1 ;;
        docker-compose|docker-compose-v2|docker-compose-plugin) ! docker compose version >/dev/null 2>&1 ;;
        docker-buildx|docker-buildx-plugin) ! docker buildx version >/dev/null 2>&1 ;;
        git|jq|unzip|file|curl) ! command -v "$1" >/dev/null 2>&1 ;;
        ffmpeg|ffmpeg-free|ffprobe) ! command -v ffmpeg >/dev/null 2>&1 || \
            ! command -v ffprobe >/dev/null 2>&1 ;;
        python) ! command -v python3 >/dev/null 2>&1 || \
            ! python3 -c 'import sys; raise SystemExit(sys.version_info < (3, 10))' >/dev/null 2>&1 ;;
        python3) ! command -v python3 >/dev/null 2>&1 || \
            ! python3 -c 'import sys; raise SystemExit(sys.version_info < (3, 10))' >/dev/null 2>&1 ;;
        python3-venv) ! command -v python3 >/dev/null 2>&1 || \
            ! python3 -m venv --help >/dev/null 2>&1 ;;
        base-devel|build-essential|gcc|make) ! c_compiler_available ;;
        *) return 1 ;;
    esac
}
filter_missing_packages() {
    local item
    local needed=()
    if ((backend_deps)) && [[ "$package_manager" == apt ]] && command -v docker >/dev/null 2>&1 && \
        command -v dpkg-query >/dev/null 2>&1; then
        if dpkg-query -W -f='${db:Status-Status}' docker-ce 2>/dev/null | grep -qx installed || \
            dpkg-query -W -f='${db:Status-Status}' docker-ce-cli 2>/dev/null | grep -qx installed; then
            replace_package docker-compose-v2 docker-compose-plugin
            replace_package docker-compose docker-compose-plugin
            replace_package docker-buildx docker-buildx-plugin
        fi
    fi
    for item in ${packages[@]+"${packages[@]}"}; do
        if package_is_needed "$item"; then needed+=("$item"); fi
    done
    packages=(${needed[@]+"${needed[@]}"})
    if ((backend_deps)) && [[ "$os_name" == Darwin && "$package_manager" == brew ]]; then
        if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
            brew_cask=0
        elif command -v docker >/dev/null 2>&1; then
            brew_cask=0
            if package_is_needed docker-compose; then packages+=(docker-compose); fi
        fi
        if command -v docker >/dev/null 2>&1 && package_is_needed docker-buildx; then
            packages+=(docker-buildx)
        fi
    fi
}
filter_dry_run_packages() {
    local item
    local planned=()
    for item in ${packages[@]+"${packages[@]}"}; do
        case "$item" in
            docker|docker.io|moby-engine)
                command -v docker >/dev/null 2>&1 || planned+=("$item") ;;
            docker-compose|docker-compose-v2|docker-compose-plugin|docker-buildx|docker-buildx-plugin)
                planned+=("$item") ;;
            *)
                if package_is_needed "$item"; then planned+=("$item"); fi ;;
        esac
    done
    packages=(${planned[@]+"${planned[@]}"})
    if ((backend_deps)) && [[ "$os_name" == Darwin && "$package_manager" == brew ]] && command -v docker >/dev/null 2>&1; then
        brew_cask=0
        packages+=(docker-compose docker-buildx)
    fi
}
print_package_plan() {
    case "$package_manager" in
        pacman) ((${#packages[@]} == 0)) || printf '  sudo pacman -S --needed %s\n' "${packages[*]}" ;;
        apt) ((${#packages[@]} == 0)) || printf '  sudo apt-get update\n  sudo apt-get install -y --no-upgrade %s\n' "${packages[*]}" ;;
        dnf) ((${#packages[@]} == 0)) || printf '  sudo dnf install -y %s\n' "${packages[*]}" ;;
        brew)
            if ((${#packages[@]})); then
                printf '  brew install %s\n' "${packages[*]}"
            fi
            (( !brew_cask )) || echo '  brew install --cask docker-desktop'
            ;;
        missing-brew)
            (( ${#packages[@]} == 0 && !brew_cask )) || echo '  install Homebrew from https://brew.sh, then rerun this command'
            ;;
    esac
    if ((source_build && rust_missing)); then
        echo '  install Rust 1.89+ for the current user with rustup (no sudo)'
    fi
    if ((source_build)) && ! c_compiler_available && [[ "$os_name" == Darwin ]]; then
        echo '  install Apple Command Line Tools with: xcode-select --install'
    fi
}

if ((dry_run)); then
    echo "Host: $os_name $host_arch${os_id:+ ($os_id)}"
    if [[ "$os_name" == Linux ]] && command -v getconf >/dev/null 2>&1; then
        echo "Host page size: $(getconf PAGESIZE) bytes"
    fi
    if ((backend_deps)); then
        echo 'Docker server profile: pending (not queried during dry run)'
    fi
    find_package_manager
    check_dependencies
    if ((dry_run)); then filter_dry_run_packages; else filter_missing_packages; fi
    add_source_installer_packages
    if [[ -n "$missing" ]]; then
        echo "Missing: $missing"
        if ((install_packages)); then
            echo 'Package changes planned (not run):'
            print_package_plan
        else
            echo 'Rerun with --install to install missing system packages.'
        fi
    else
        echo 'Required commands are present (service access is not checked during dry run).'
    fi
    (( !start_docker )) || echo 'Docker service/Desktop startup would happen after installation; no Docker call was made.'
    exit 0
fi

find_package_manager
check_dependencies
filter_missing_packages
add_source_installer_packages
if [[ -n "$missing" ]]; then
    if ((!install_packages)); then
        if ((source_build)) && ! c_compiler_available && [[ "$os_name" == Darwin ]]; then
            die 'Apple Command Line Tools are required for a source build; install them with xcode-select --install, then rerun'
        fi
        echo "Missing prerequisites: $missing" >&2
        echo 'Rerun with --install-deps to install them.' >&2
        exit 1
    fi
    [[ "$package_manager" != missing-brew ]] || die 'Homebrew is required to install macOS dependencies; install it from https://brew.sh and rerun'
    echo 'This will install the following host packages:'
    print_package_plan
    if ((backend_deps)) && [[ "$os_name" == Linux ]]; then
        cat <<'EOF'
Docker group membership grants root-equivalent access to the local machine.
The installer will add the current user to that group only if a local rootful
Docker socket is inaccessible after package installation.
EOF
    fi
    if ((!assume_yes)); then
        [[ -t 0 ]] || die 'no terminal for confirmation; rerun with --yes to accept the package plan'
        printf 'Continue with these changes? [y/N] '
        read -r answer
        case "$answer" in y|Y|yes|YES) ;; *) die 'cancelled before package changes' ;; esac
    fi
    if [[ "$os_name" != Darwin && "$EUID" -ne 0 ]]; then
        command -v sudo >/dev/null 2>&1 || die 'sudo is required to install host packages'
        sudo -v || die 'could not obtain administrator access for package installation'
    fi
    if ((${#packages[@]} || brew_cask)); then
        case "$package_manager" in
            pacman)
                if [[ "$EUID" -eq 0 ]]; then pacman -S --needed --noconfirm ${packages[@]+"${packages[@]}"}
                else sudo pacman -S --needed --noconfirm ${packages[@]+"${packages[@]}"}; fi ;;
            apt)
                if [[ "$EUID" -eq 0 ]]; then apt-get update; apt-get install -y --no-upgrade ${packages[@]+"${packages[@]}"}
                else sudo apt-get update; sudo apt-get install -y --no-upgrade ${packages[@]+"${packages[@]}"}; fi ;;
            dnf)
                if [[ "$EUID" -eq 0 ]]; then dnf install -y ${packages[@]+"${packages[@]}"}
                else sudo dnf install -y ${packages[@]+"${packages[@]}"}; fi ;;
            brew)
                if ((${#packages[@]})); then
                    HOMEBREW_NO_AUTO_UPDATE=1 brew install ${packages[@]+"${packages[@]}"}
                fi
                (( !brew_cask )) || HOMEBREW_NO_AUTO_UPDATE=1 brew install --cask docker-desktop
                if [[ "$os_name" == Darwin ]]; then
                    for item in ${packages[@]+"${packages[@]}"}; do
                        case "$item" in docker-compose|docker-buildx) link_homebrew_docker_plugins; break ;; esac
                    done
                fi ;;
        esac
    fi
fi

if ((source_build)) && ! rust_is_sufficient; then
    if command -v rustup >/dev/null 2>&1; then
        rustup toolchain install stable --profile minimal
    else
        command -v curl >/dev/null 2>&1 || die 'curl is required to install Rust 1.89+'
        rustup_installer=$(mktemp "${TMPDIR:-/tmp}/ipod-rustup.XXXXXX")
        trap 'rm -f "$rustup_installer"' EXIT
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$rustup_installer"
        sh "$rustup_installer" -y --profile minimal --default-toolchain stable --no-modify-path
    fi
    PATH="${HOME:?HOME is required}/.cargo/bin:$PATH"
    export PATH
    rust_is_sufficient || die 'Rust/Cargo 1.89+ is still unavailable after setup'
fi

local_default_docker() {
    local context
    [[ -z "${DOCKER_HOST:-}" ]] || return 1
    context=$(docker context show 2>/dev/null || echo default)
    [[ "$context" == default || ( "$os_name" == Darwin && "$context" == desktop-linux ) ]]
}
docker_group_added=0
docker_info_works() {
    command -v docker >/dev/null 2>&1 || return 1
    if ((docker_group_added)); then
        sg docker -c 'docker compose version && docker info' >/dev/null 2>&1
    else
        docker compose version >/dev/null 2>&1 && docker info >/dev/null 2>&1
    fi
}
grant_docker_group() {
    ((install_packages)) && [[ "$EUID" -ne 0 ]] || return 1
    sudo -v || die 'sudo is required to grant local Docker access'
    sudo groupadd -f docker
    sudo usermod -aG docker "${USER:?USER is required}"
    command -v sg >/dev/null 2>&1 || die 'sg is unavailable; open a new login shell and rerun the install command'
    sg docker -c 'id -nG' | tr ' ' '\n' | grep -qx docker || \
        die 'docker group membership was added but could not be activated in this shell; open a new login shell and rerun'
    docker_group_added=1
    echo 'Added your user to the docker group. The group is root-equivalent.'
    echo 'The current installer command can use the new membership through sg; new shells inherit it automatically.'
}
socket_inaccessible() {
    [[ "$os_name" == Linux && -S "$docker_socket_path" ]] &&
        [[ ! -r "$docker_socket_path" || ! -w "$docker_socket_path" ]]
}
if ((backend_deps)) && local_default_docker && socket_inaccessible; then
    if ! grant_docker_group && ((start_docker)); then
        die 'Docker socket permissions prevent access; rerun with --install-deps to grant local docker-group access or configure rootless Docker'
    fi
fi

if ((start_docker)) && ! docker_info_works; then
    local_default_docker || die 'selected Docker context is inaccessible; start that rootless/remote daemon without changing context'
    if [[ "$os_name" == Darwin ]]; then
        command -v open >/dev/null 2>&1 || die 'open was not found; start Docker Desktop manually'
        open -a Docker || die 'Docker Desktop could not be opened; start it manually'
    else
        if [[ "$EUID" -ne 0 ]]; then sudo -v || die 'sudo is required to start the local Docker service'; fi
        if command -v systemctl >/dev/null 2>&1; then
            if [[ "$EUID" -eq 0 ]]; then systemctl start docker; else sudo systemctl start docker; fi
        elif command -v service >/dev/null 2>&1; then
            if [[ "$EUID" -eq 0 ]]; then service docker start; else sudo service docker start; fi
        else
            die 'no system service manager found; start Docker manually'
        fi
    fi
    ready=0
    for ((attempt=0; attempt < 60; attempt++)); do
        if socket_inaccessible && ((!docker_group_added)); then
            if grant_docker_group; then
                :
            else
                die 'Docker socket permissions prevent access; rerun with --install-deps to grant local docker-group access or configure rootless Docker'
            fi
        fi
        if docker_info_works; then ready=1; break; fi
        sleep 2
    done
    ((ready)) || die 'Docker did not become ready within 120 seconds; start Docker manually and retry'
fi

check_dependencies
if [[ -n "$missing" && "$os_id" == debian && "$missing" == *'Docker Compose v2'* ]]; then
    die "Debian before version 13 does not provide the required Compose v2 plugin in its default repository; install Docker Compose v2 from Docker's official Debian repository, then rerun"
fi
if [[ -n "$missing" && "$os_name" == Darwin ]] && ((source_build)) && ! c_compiler_available; then
    die 'Apple Command Line Tools are required for a source build; install them with xcode-select --install, then rerun'
fi
[[ -z "$missing" ]] || die "dependencies remain unavailable after setup: $missing"
echo 'Host dependency checks passed.'
