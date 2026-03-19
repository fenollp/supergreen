pub(crate) const VERSION: &str = "1.29.0";

pub(crate) static CHECKSUMS: phf::Map<&'static str, &'static str> = phf::phf_map! {
    "aarch64-apple-darwin"      => "aeb4105778ca1bd3c6b0e75768f581c656633cd51368fa61289b6a71696ac7e1",
    "aarch64-unknown-linux-gnu" => "9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792",
    "x86_64-unknown-linux-gnu"  => "4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10",
};
