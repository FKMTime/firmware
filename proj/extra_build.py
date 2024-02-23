Import("env")
# from collections import deque
# import re
# import os

# def insert_firmware_version(env, node):
#     build_path = node.get_abspath().replace(os.getcwd(), "")
#     build_path = re.sub("/.pio/build/(.*?)/", "", build_path)

#     if build_path == 'src/main.cpp':
#         return env.Object(
#             node,
#             CPPDEFINES=deque(env["CPPDEFINES"]) + deque([("FIRMWARE_VERSION", "321")]),
#             CCFLAGS=deque(env["CCFLAGS"])
#         )
#     return node

# env.AddBuildMiddleware(insert_firmware_version)

import os, time

def after_build(source, target, env):
    version = os.popen("cat src/version.h | grep \"FIRMWARE_VERSION\" | cut -d'\"' -f 2").read().strip()
    buildTime = os.popen("cat src/version.h | grep \"BUILD_TIME\" | cut -d'\"' -f 2").read().strip()
    bin_name = f"{env['BOARD_MCU']}.{version}.{buildTime}.bin"
    os.popen(f"mkdir -p ./build ; cp {source[0].get_abspath()} ./build/{bin_name}")

def generate_version():
    filesHash = os.popen("find ./platformio.ini ./src ./lib ./include -type f -print0 | sort -fdz | xargs -0 sha1sum | grep -v ./src/version.h | sha1sum | awk '{print $1}'").read().strip()
    try:
        with open(".versum", "r") as file:
            if filesHash == file.read().strip():
                return
    except:
        print(".versum doesn't exists! Building...")

    version = filesHash[:8]
    buildTime = format(int(time.time()), 'x')
    versionPath = os.path.join(env["PROJECT_DIR"], "src", "version.h")
    versionString = """
#ifndef __VERSION_H__
#define __VERSION_H__

#define FIRMWARE_VERSION "{version}"
#define BUILD_TIME "{buildTime}"

#endif
""".format(version = version, buildTime = buildTime)

    with open(versionPath, "w") as file:
        file.write(versionString)
    with open(".versum", "w") as file:
        file.write(filesHash.strip())

env.AddPostAction("buildprog", after_build)
generate_version()
