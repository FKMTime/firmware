Import("env")
import os, time, os

def after_build(source, target, env):
    version = os.popen("cat src/version.h | grep \"FIRMWARE_VERSION\" | cut -d'\"' -f 2").read().strip()
    buildTime = os.popen("cat src/version.h | grep \"BUILD_TIME\" | cut -d'\"' -f 2").read().strip()
    firmwareType = os.popen("cat src/version.h | grep \"FIRMWARE_TYPE\" | cut -d'\"' -f 2").read().strip()
    chip = env['BOARD_MCU']

    os.popen(f"mkdir -p ../build ; cp {source[0].get_abspath()} ../build/{chip}_{firmwareType}_{version}.bin")

def generate_version():
    release_build = "RELEASE_BUILD" in env["ENV"]

    buildTime = int(time.time())
    version = release_build == True and env["ENV"]["RELEASE_BUILD"] or ("DV" + str(buildTime))
    versionPath = os.path.join(env["PROJECT_DIR"], "src", "version.h")
    chip = env['BOARD_MCU']

    print(f"Version: {version}")
    print(f"Build Time: {buildTime}")
    print(f"Chip: {chip}")

    versionString = """
#ifndef __VERSION_H__
#define __VERSION_H__

#define FIRMWARE_VERSION "{version}"
#define BUILD_TIME "{buildTime}"
#define FIRMWARE_TYPE "{firmwareType}"
#define CHIP "{chip}"

#endif
""".format(version = version, buildTime = buildTime, chip = chip, firmwareType = "STATION" )

    with open(versionPath, "w") as file:
        file.write(versionString)

env.AddPostAction("buildprog", after_build)
generate_version()
