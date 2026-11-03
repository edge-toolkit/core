# bluetooth

The `bluetooth` module lets the participant pick a nearby Bluetooth Low Energy device through the
[Web Bluetooth API](https://developer.mozilla.org/docs/Web/API/Web_Bluetooth_API). Its `run()` calls
`navigator.bluetooth.requestDevice` with `acceptAllDevices`, so the browser shows its own device picker, then reports
the chosen device's id and name to the hub as a `device_selected` client event; it does not connect to the device.
`BluetoothAccess` also exposes `connectGatt`, which opens the device's GATT server, for a module that needs one.

## iPhone and iPad: no standard browser can run it

Web Bluetooth does not exist in Safari, nor in any other standard browser on an iPhone or iPad, so this module cannot
run there in Safari, Chrome, Firefox or Edge. It stops with

```text
Web Bluetooth is not available in this browser context
```

Three independent sources establish it:

- **Safari has no Web Bluetooth.** MDN's browser compatibility data marks it unsupported in Safari on macOS and iOS,
  and in the iOS WebView that iPhone apps embed ([mdn/browser-compat-data `api/Bluetooth.json`][bcd]).
- **Apple does not intend to add it.** WebKit recorded a formal `oppose` position on 2 December 2025, citing
  security, privacy and the web's device independence ([WebKit/standards-positions#570][webkit]).
- **Every iPhone browser runs Safari's engine.** App Store Review Guideline 2.5.6 requires every app that browses
  the web to use WebKit, so Chrome, Firefox and Edge on an iPhone inherit Safari's gap ([App Store Review
  Guidelines][guidelines]). Apple lets a browser apply to use another engine only in the EU and Japan, so everywhere
  else, Australia included, the limit holds for every standard iPhone browser.

## Workaround on iPhone and iPad: Bluefy

[Bluefy -- Web BLE Browser][bluefy] is a free App Store browser that adds Web Bluetooth to the pages it opens. A
participant who installs it and opens the hub's page in Bluefy rather than Safari can run this module. Plan for it
before recruiting iPhone users:

- Serve the page over HTTPS. Web Bluetooth is only exposed to secure contexts, and Bluefy's App Store listing says
  the same, so give iPhone participants the hub's TLS address.
- Bluefy is a third-party app, not part of edge-toolkit or iOS. Test the study's page in it end to end, at the
  address participants will use, and tell participants in their instructions that they need it.

[bcd]: https://github.com/mdn/browser-compat-data/blob/main/api/Bluetooth.json
[webkit]: https://github.com/WebKit/standards-positions/issues/570
[guidelines]: https://developer.apple.com/app-store/review/guidelines/
[bluefy]: https://apps.apple.com/app/id1492822055
