// cidre pulls in a Swift-concurrency-linked static library that references
// `@rpath/libswift_Concurrency.dylib`. On this machine that dylib is not a file
// on disk -- it lives in the dyld shared cache -- so the binary dies at launch
// with "no LC_RPATH's found" unless an rpath points at the Swift runtime
// directory. Recorded as a finding in docs/AUDIO-TAP.md (risk R6).
fn main() {
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
