Import("env")
import os, time, os

OTA_PROJ_NAME = "fkm"

def curl_get_ota_version(project, channel, chip):
    res = os.popen(f"curl -s https://ota.filipton.space/firmware/{project}/{channel}/{chip}/latest.json").read()
    version = os.popen(f"echo '{res}' | jq -r .version").read().strip()

    return version

def after_build(source, target, env):
    version = os.popen("cat src/version.h | grep \"FIRMWARE_VERSION\" | cut -d'\"' -f 2").read().strip()
    buildTime = os.popen("cat src/version.h | grep \"BUILD_TIME\" | cut -d'\"' -f 2").read().strip()
    firmwareType = os.popen("cat src/version.h | grep \"FIRMWARE_TYPE\" | cut -d'\"' -f 2").read().strip()
    bin_name = f"{env['BOARD_MCU']}.{firmwareType}.{version}.{buildTime}.bin"
    os.popen(f"mkdir -p ../build ; cp {source[0].get_abspath()} ../build/{bin_name}")

def generate_version():
    release_build = "RELEASE_BUILD" in env["ENV"]
    filesHash = os.popen("bash ./hash.sh").read().strip()
    try:
        with open(".versum", "r") as file:
            if filesHash == file.read().strip():
                return
    except:
        print(".versum doesn't exists! Building...")

    version = filesHash[:8]
    buildTime = format(int(time.time()), 'x')
    versionPath = os.path.join(env["PROJECT_DIR"], "src", "version.h")
    chip = env['BOARD_MCU']
    channel = "prerelease"

    if release_build == True:
        version = os.popen("cat ./VERSION").read().strip()
        channel = "stable"

    print(f"Version: {version}")
    print(f"Build Time: {buildTime}")
    print(f"Chip: {chip}")
    print(f"Channel: {channel}")

    curl_version = curl_get_ota_version(OTA_PROJ_NAME, channel, chip)
    print(f"OTA Version: {curl_version}")

    env.Exit(0)

    versionString = """
#ifndef __VERSION_H__
#define __VERSION_H__

#define FIRMWARE_VERSION "{version}"
#define BUILD_TIME "{buildTime}"
#define FIRMWARE_TYPE "STATION"
#define CHIP "{chip}"

#endif
""".format(version = version, buildTime = buildTime, chip = chip)

    with open(versionPath, "w") as file:
        file.write(versionString)
    with open(".versum", "w") as file:
        file.write(filesHash.strip())

env.AddPostAction("buildprog", after_build)
generate_version()
