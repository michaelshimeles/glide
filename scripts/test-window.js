// A plain app window from another process (osascript) for the selftest's
// audio checks: an app ScreenCaptureKit can hear, that isn't DaisyCap.
// Usage: osascript -l JavaScript test-window.js SECONDS [SOUND DELAY]
// shows the window for SECONDS and, given SOUND, plays it after DELAY s.
ObjC.import('Cocoa');
ObjC.import('AVFoundation');
function run(argv) {
  var secs = parseFloat(argv[0] || '30');
  var app = $.NSApplication.sharedApplication;
  app.setActivationPolicy($.NSApplicationActivationPolicyAccessory);
  var w = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer($.NSMakeRect(40, 40, 360, 220), 1, $.NSBackingStoreBuffered, false);
  w.title = argv[1] ? 'DaisyCap audio test (sound)' : 'DaisyCap audio test';
  w.backgroundColor = $.NSColor.systemTealColor;
  w.orderFrontRegardless;
  var loop = $.NSRunLoop.currentRunLoop;
  var until = function (s) { loop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(s)); };
  if (argv[1]) {
    var delay = parseFloat(argv[2] || '0');
    until(delay);
    var player = $.AVAudioPlayer.alloc.initWithContentsOfURLError($.NSURL.fileURLWithPath(argv[1]), null);
    player.play;
    until(Math.max(secs - delay, 0));
  } else {
    until(secs);
  }
}
