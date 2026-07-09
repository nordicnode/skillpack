package main
import ("os"; "fmt")
func main() {
 if len(os.Args) > 1 && os.Args[1] == "--help" { fmt.Println("Usage: sample-go [--lint] [--fix]"); return }
 fmt.Println("sample-go")
}
