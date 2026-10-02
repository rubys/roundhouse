"""A lane that runs ci.yml's steps from a writable copy of the repository."""

load("@rules_shell//shell:sh_test.bzl", "sh_test")
load("//bazel:images.bzl", "CI_IMAGE")

def repo_lane(base, name, script, args = [], data = [], spinel = False, campfire = False, env = {}, exec_properties = {}, tags = []):
    opts = []
    if spinel:
        opts += ["--spinel", "$(rootpath //:spinel_dist)"]
        data = data + ["//:spinel_dist"]
    if campfire:
        opts += ["--campfire", "$(rootpath //:campfire_src)"]
        data = data + ["//:campfire_src"]
    sh_test(
        name = name,
        srcs = ["//bazel:with_repo.sh"],
        args = opts + ["--", "bazel/lanes/%s.sh" % script] + args,
        data = base["data"] + data + ["//bazel:lanes/%s.sh" % script],
        env = dict(base["env"], **env),
        exec_properties = dict({"container-image": CI_IMAGE, "network": "external"}, **exec_properties),
        tags = ["requires-network", "lane", "needs-source-fixtures"] + tags,
        timeout = "eternal",
    )
