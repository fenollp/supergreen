group "default" {
  targets = [
    "buildxargs",
    "edit.dwp",
    "zero_reallocs.sh",
    "cargo_kani",
    "CreuSAT",
    "vixargs",
    "rg",
    "get",
    "gifski",
    "cargo_fuzz",
    "cargo_llvm_cov",
    "cargo_rail",
    "dbcc",
    "fargo",
    "a_mir_formality",
    "cross",
    "flamegraph",
    "rqcow2",
    "verso",
    "ntp_daemon",
    "mussh",
    "cargo_tally",
    "cargo_osdk",
    "gst_webrtc_signalling_server",
    "shpool",
    "qair",
    "diesel",
    "btm",
    "cargo_mutants",
    "hickory_dns",
    "alacritty",
    "rublk",
    "cargo_make",
    "binsider",
    "cargo_authors",
    "cargo_audit",
    "cargo_deny",
    "sccache",
    "tract",
    "cargo_udeps",
    "cfr",
    "cargo_nextest",
    "harper_ls",
    "ipa",
    "stu",
    "topiary",
    "torrust_index",
  ]
}

target "buildxargs" {
  context = "recipes"
  dockerfile = "buildxargs@master.Dockerfile"
  output = ["."]
}
target "edit.dwp" {
  context = "recipes"
  dockerfile = "edit@main.Dockerfile"
  output = ["."]
}
target "zero_reallocs.sh" {
  context = "recipes"
  dockerfile = "pyrefly@main.Dockerfile"
  output = ["."]
}
target "cargo_kani" {
  context = "recipes"
  dockerfile = "kani-verifier@0.66.0.Dockerfile"
  output = ["."]
}
target "CreuSAT" {
  context = "recipes"
  dockerfile = "CreuSAT@master.Dockerfile"
  output = ["."]
}
target "vixargs" {
  context = "recipes"
  dockerfile = "vixargs@0.1.0.Dockerfile"
  output = ["."]
}
target "rg" {
  context = "recipes"
  dockerfile = "ripgrep@15.1.0.Dockerfile"
  output = ["."]
}
target "get" {
  context = "recipes"
  dockerfile = "cargo-config2@0.1.39.Dockerfile"
  output = ["."]
}
target "gifski" {
  context = "recipes"
  dockerfile = "gifski@1.34.0.Dockerfile"
  output = ["."]
}
target "cargo_fuzz" {
  context = "recipes"
  dockerfile = "cargo-fuzz@0.13.1.Dockerfile"
  output = ["."]
}
target "cargo_llvm_cov" {
  context = "recipes"
  dockerfile = "cargo-llvm-cov@0.6.21.Dockerfile"
  output = ["."]
}
target "cargo_rail" {
  context = "recipes"
  dockerfile = "cargo-rail@0.1.0.Dockerfile"
  output = ["."]
}
target "dbcc" {
  context = "recipes"
  dockerfile = "dbcc@2.2.1.Dockerfile"
  output = ["."]
}
target "fargo" {
  context = "recipes"
  dockerfile = "fargo@main.Dockerfile"
  output = ["."]
}
target "a_mir_formality" {
  context = "recipes"
  dockerfile = "a-mir-formality@main.Dockerfile"
  output = ["."]
}
target "cross" {
  context = "recipes"
  dockerfile = "cross@0.2.5.Dockerfile"
  output = ["."]
}
target "flamegraph" {
  context = "recipes"
  dockerfile = "flamegraph@0.6.10.Dockerfile"
  output = ["."]
}
target "rqcow2" {
  context = "recipes"
  dockerfile = "qcow2-rs@0.1.6.Dockerfile"
  output = ["."]
}
target "verso" {
  context = "recipes"
  dockerfile = "verso@main.Dockerfile"
  output = ["."]
}
target "ntp_daemon" {
  context = "recipes"
  dockerfile = "ntpd@1.7.1.Dockerfile"
  output = ["."]
}
target "mussh" {
  context = "recipes"
  dockerfile = "mussh@3.1.3.Dockerfile"
  output = ["."]
}
target "cargo_tally" {
  context = "recipes"
  dockerfile = "cargo-tally@1.0.71.Dockerfile"
  output = ["."]
}
target "cargo_osdk" {
  context = "recipes"
  dockerfile = "cargo-osdk@main.Dockerfile"
  output = ["."]
}
target "gst_webrtc_signalling_server" {
  context = "recipes"
  dockerfile = "gst-plugin-webrtc-signalling@main.Dockerfile"
  output = ["."]
}
target "shpool" {
  context = "recipes"
  dockerfile = "shpool@0.9.3.Dockerfile"
  output = ["."]
}
target "qair" {
  context = "recipes"
  dockerfile = "qair@main.Dockerfile"
  output = ["."]
}
target "diesel" {
  context = "recipes"
  dockerfile = "diesel_cli@2.3.4.Dockerfile"
  output = ["."]
}
target "btm" {
  context = "recipes"
  dockerfile = "bottom@0.11.4.Dockerfile"
  output = ["."]
}
target "cargo_mutants" {
  context = "recipes"
  dockerfile = "cargo-mutants@25.3.1.Dockerfile"
  output = ["."]
}
target "hickory_dns" {
  context = "recipes"
  dockerfile = "hickory-dns@0.26.0-alpha.1.Dockerfile"
  output = ["."]
}
target "alacritty" {
  context = "recipes"
  dockerfile = "alacritty@0.17.0.Dockerfile"
  output = ["."]
}
target "rublk" {
  context = "recipes"
  dockerfile = "rublk@0.2.13.Dockerfile"
  output = ["."]
}
target "cargo_make" {
  context = "recipes"
  dockerfile = "cargo-make@0.37.24.Dockerfile"
  output = ["."]
}
target "binsider" {
  context = "recipes"
  dockerfile = "binsider@0.3.0.Dockerfile"
  output = ["."]
}
target "cargo_authors" {
  context = "recipes"
  dockerfile = "cargo-authors@0.5.5.Dockerfile"
  output = ["."]
}
target "cargo_audit" {
  context = "recipes"
  dockerfile = "cargo-audit@0.22.0.Dockerfile"
  output = ["."]
}
target "cargo_deny" {
  context = "recipes"
  dockerfile = "cargo-deny@0.18.5.Dockerfile"
  output = ["."]
}
target "sccache" {
  context = "recipes"
  dockerfile = "sccache@0.12.0.Dockerfile"
  output = ["."]
}
target "tract" {
  context = "recipes"
  dockerfile = "tract@0.22.1.Dockerfile"
  output = ["."]
}
target "cargo_udeps" {
  context = "recipes"
  dockerfile = "cargo-udeps@0.1.60.Dockerfile"
  output = ["."]
}
target "cfr" {
  context = "recipes"
  dockerfile = "coccinelleforrust@main.Dockerfile"
  output = ["."]
}
target "cargo_nextest" {
  context = "recipes"
  dockerfile = "cargo-nextest@0.9.114.Dockerfile"
  output = ["."]
}
target "harper_ls" {
  context = "recipes"
  dockerfile = "harper@master.Dockerfile"
  output = ["."]
}
target "ipa" {
  context = "recipes"
  dockerfile = "ipa@main.Dockerfile"
  output = ["."]
}
target "stu" {
  context = "recipes"
  dockerfile = "stu@0.7.5.Dockerfile"
  output = ["."]
}
target "topiary" {
  context = "recipes"
  dockerfile = "topiary-cli@0.7.3.Dockerfile"
  output = ["."]
}
target "torrust_index" {
  context = "recipes"
  dockerfile = "torrust-index@4.0.0-develop.Dockerfile"
  output = ["."]
}
