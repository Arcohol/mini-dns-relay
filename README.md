# mini-dns-relay

## How to compile

```
cargo build --release
```

## How to run

```
sudo ./target/release/mini-dns-relay -vv
```

## Usage

```
./target/release/mini-dns-relay --help
```

There are FOUR levels of logging. Applying no `-v` will result in showing only errors.
- `-v` - INFO
- `-vv` - DEBUG
- `-vvv` - TRACE
