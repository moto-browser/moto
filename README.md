# Moto

Moto is an experimental browser based on the Servo browser engine.

## Setting Expectations

- This is currently being worked on by one person, on the side, in their free time.
- This builds off the existing Servoshell minibrowser.
- Focus will be mainly on desktop for now, as Servo's Android support is still somewhat nascent and it's a bit slower to iterate on.

## Short-Term Goals

Experimenting with implementing features to see what a more developed, user-focused browser for Servo could look like. These include things like:

- Favorites/Bookmarks
- History
- Saved passwords/Autofill
- Download Managing
- WebExtensions
- And more!

## Long-Term Goals

There's not really much set in stone! As experimentation here continues, the hope is that it will help inform the development of Servo's embedding API, much in the same way that projects like [Verso](https://github.com/versotile-org/verso) or [Cuervo](https://github.com/mcclure/cuervo) already are.

## Development

See [Servo's requirements for building](https://book.servo.org/hacking/setting-up-your-environment.html). Otherwise you should just be able to do regular `cargo build` and `cargo run`.
