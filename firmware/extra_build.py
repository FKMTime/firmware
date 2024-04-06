Import("env")
import os, time, os

OTA_PROJ_NAME = "fkm"

def curl_get_ota_version(project, channel, chip):
    res = os.popen(f"curl -s https://ota.filipton.space/firmware/{project}/{channel}/{chip}/latest.json").read()
    print("OTA response:", res)
    version = os.popen(f"echo '{res}' | jq -r .version").read().strip()

    return version

def curl_upload_ota(project, channel, chip, version, name, bin, token):
    print(f"Uploading {bin} to OTA")
    print(f"Url: https://ota.filipton.space/latest/{project}/{channel}/{chip}/{version}/{name}.bin")
    os.popen(f"curl -s -T {bin} -H 'Authorization: {token}' https://ota.filipton.space/latest/{project}/{channel}/{chip}/{version}/{name}.bin")

def after_build(source, target, env):
    release_build = "RELEASE_BUILD" in env["ENV"]

    version = os.popen("cat src/version.h | grep \"FIRMWARE_VERSION\" | cut -d'\"' -f 2").read().strip()
    # buildTime = os.popen("cat src/version.h | grep \"BUILD_TIME\" | cut -d'\"' -f 2").read().strip()
    firmwareType = os.popen("cat src/version.h | grep \"FIRMWARE_TYPE\" | cut -d'\"' -f 2").read().strip()
    chip = env['BOARD_MCU']
    channel = release_build == True and "stable" or "prerelease"
    name = release_build == True and version or int(time.time())

    if "OTA_TOKEN" in env["ENV"]:
        curl_upload_ota(firmwareType, channel, chip, version, name, source[0].get_abspath(), env["ENV"]["OTA_TOKEN"])
    else:
        os.popen(f"mkdir -p ../build ; rm -f ../build/{chip}.{firmwareType}.*.bin ; cp {source[0].get_abspath()} ../build/{chip}.{firmwareType}.{version}.bin")
        print("OTA_TOKEN is not set! Skipping OTA upload")

def generate_version():
    release_build = "RELEASE_BUILD" in env["ENV"]
    filesHash = os.popen("bash ./hash.sh").read().strip()

    if not release_build:
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
    channel = release_build == True and "stable" or "prerelease"
    curl_version = curl_get_ota_version(OTA_PROJ_NAME, channel, chip).strip()

    if release_build == True:
        print("Previous version:", curl_version)
        print("Enter new version:")
        version = input()
    elif "OTA_TOKEN" not in env["ENV"]:
        version = int(time.time())

    print(f"Version: {version}")
    print(f"Build Time: {buildTime}")
    print(f"Chip: {chip}")
    print(f"Channel: {channel}")
    print(f"OTA Version: {curl_version}")

    if version == curl_version:
        print("Version is up to date")
        env.Exit(0)

    versionString = """
#ifndef __VERSION_H__
#define __VERSION_H__

#define FIRMWARE_VERSION "{version}"
#define BUILD_TIME "{buildTime}"
#define FIRMWARE_TYPE "{otaProjName}"
#define CHIP "{chip}"

#endif
""".format(version = version, buildTime = buildTime, chip = chip, otaProjName = OTA_PROJ_NAME)

    with open(versionPath, "w") as file:
        file.write(versionString)
    with open(".versum", "w") as file:
        file.write(filesHash.strip())

env.AddPostAction("buildprog", after_build)
generate_version()
