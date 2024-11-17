Import("env")
import os, time, os

def after_build(source, target, env):
    version = os.popen("cat src/version.h | grep \"FIRMWARE_VERSION\" | cut -d'\"' -f 2").read().strip()
    firmwareType = os.popen("cat src/version.h | grep \"FIRMWARE_TYPE\" | cut -d'\"' -f 2").read().strip()
    chip = env['BOARD_MCU']

    os.popen(f"mkdir -p /tmp/fkm-build ; cp {source[0].get_abspath()} /tmp/fkm-build/{chip}_{firmwareType}_{version}.bin")

def generate_version():
    release_build = "RELEASE_BUILD" in env["ENV"]

    buildTime = int(time.time())
    version = release_build == True and env["ENV"]["RELEASE_BUILD"] or ("D" + str(buildTime))
    versionPath = os.path.join(env["PROJECT_DIR"], "src", "version.h")
    chip = env['BOARD_MCU']

    print(f"Version: {version}")
    print(f"Chip: {chip}")

    versionString = """
#ifndef __VERSION_H__
#define __VERSION_H__

#define FIRMWARE_VERSION "{version}"
#define FIRMWARE_TYPE "{firmwareType}"
#define CHIP "{chip}"

#endif
""".format(version = version, chip = chip, firmwareType = "STATION" )

    with open(versionPath, "w") as file:
        file.write(versionString)

env.AddPostAction("buildprog", after_build)
generate_version()
