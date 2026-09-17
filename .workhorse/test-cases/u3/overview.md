# Application inherits machine warning status incorrectly

Scenarios verifying that each grain is graded on its own checks.

## Health rollup

- [x] An application whose own checks all pass reads healthy while its machine has a failing check, and the machine reads unhealthy (verifies spec: CHK)
- [x] The machine's check is still on the application's consolidated list, marked as the machine's, with its own effective result (verifies spec: CHK)
- [x] An application with a failing check of its own reads unhealthy whatever its machine's state (verifies spec: CHK)

## Operator surfaces

- [ ] On an application's detail page, a machine-only warning leaves the headline chip healthy and appears in the checks table marked as the machine's
- [ ] On the status page, a machine-only warning colours the machine's enclosure and leaves the dots of the applications on it healthy
- [ ] The MCP `get_server` and `get_machine` answers agree with what those two surfaces show for the same box
