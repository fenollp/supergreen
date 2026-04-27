group "default" {
  targets = [
    "buildxargs",
    "cargo_kani",
    "vixargs",
    "rg",
    "get",
    "gifski",
    "cargo_llvm_cov",
    "cargo_rail",
    "dbcc",
    "cross",
    "flamegraph",
    "rqcow2",
    "ntp_daemon",
    "mussh",
    "cargo_tally",
    "shpool",
    "diesel",
    "btm",
    "cargo_mutants",
    "hickory_dns",
    "alacritty",
    "rublk",
    "binsider",
    "cargo_authors",
    "cargo_deny",
    "sccache",
    "cargo_nextest",
    "stu",
    "topiary",
  ]
}

target "buildxargs" {
  context = "recipes"
  dockerfile = "buildxargs@master.Dockerfile"
  output = ["."]
}
target "cargo_kani" {
  context = "recipes"
  dockerfile = "kani-verifier@0.66.0.Dockerfile"
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
target "shpool" {
  context = "recipes"
  dockerfile = "shpool@0.9.3.Dockerfile"
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
target "cargo_nextest" {
  context = "recipes"
  dockerfile = "cargo-nextest@0.9.114.Dockerfile"
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
