// A code viewer must preserve source, never execute it.
fn main() {
    let literal = "<script>window.injected = true</script> & λ";
    println!("{literal}");
}
