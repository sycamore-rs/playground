(function () {
  "use strict";

  var MAX_TEXT_LENGTH = 4000;
  var MAX_ARGUMENTS = 20;
  var MAX_MESSAGES = 200;
  var sentMessages = 0;
  var forwardingStopped = false;
  var counts = new Map();
  var timers = new Map();

  function truncate(text) {
    if (text.length <= MAX_TEXT_LENGTH) return text;
    return text.slice(0, MAX_TEXT_LENGTH - 1) + "…";
  }

  function formatValue(value) {
    if (value === undefined) return "undefined";
    if (value === null) return "null";
    if (typeof value === "string") return value;
    if (typeof value === "bigint") return String(value) + "n";
    if (typeof value === "symbol") return String(value);
    if (typeof value === "function") {
      try {
        return String(value);
      } catch (_) {
        return "[Function]";
      }
    }
    if (typeof value !== "object") return String(value);

    try {
      if (value instanceof Error) {
        var name = value.name;
        var message = value.message;
        var stack = value.stack;
        if (stack) return String(stack);
        var errorText = String(name || "Error");
        if (message) errorText += ": " + String(message);
        return errorText;
      }
    } catch (_) {
      // Fall through to JSON serialization, which is also guarded.
    }

    try {
      var seen = new WeakSet();
      var serialized = JSON.stringify(value, function (_key, item) {
        if (typeof item === "bigint") return String(item) + "n";
        if (typeof item === "undefined") return "[undefined]";
        if (typeof item === "symbol") return String(item);
        if (typeof item === "function") return String(item);
        if (item instanceof Error) {
          var error = {};
          try { error.name = item.name; } catch (_) { error.name = "Error"; }
          try { error.message = item.message; } catch (_) { error.message = "[unavailable]"; }
          try { error.stack = item.stack; } catch (_) { /* omit inaccessible stack */ }
          return error;
        }
        if (item && typeof item === "object") {
          if (seen.has(item)) return "[Circular]";
          seen.add(item);
        }
        return item;
      });
      return serialized === undefined ? String(value) : serialized;
    } catch (_) {
      return "[Unserializable object]";
    }
  }

  function formatArguments(args) {
    try {
      var length = Math.min(args.length, MAX_ARGUMENTS);
      var parts = [];
      for (var i = 0; i < length; i++) {
        try {
          parts.push(formatValue(args[i]));
        } catch (_) {
          parts.push("[Unserializable value]");
        }
      }
      if (args.length > MAX_ARGUMENTS) {
        parts.push("… (" + (args.length - MAX_ARGUMENTS) + " more arguments)");
      }
      return truncate(parts.join(" "));
    } catch (_) {
      return "[Unable to format console output]";
    }
  }

  function post(level, text) {
    if (forwardingStopped) return;

    // Reserve the final slot for the single limit notice, so at most 200
    // messages (including that notice) are sent during one iframe run.
    if (sentMessages >= MAX_MESSAGES - 1) {
      send("warn", "Console message limit reached (200 messages); further output is suppressed.");
      forwardingStopped = true;
      return;
    }
    send(level, text);
  }

  function send(level, text) {
    if (sentMessages >= MAX_MESSAGES) return;
    try {
      var payload = JSON.stringify({
        type: "sycamore-playground-console",
        level: level,
        text: truncate(String(text))
      });
      globalThis.parent.postMessage(payload, "*");
      sentMessages++;
    } catch (_) {
      // The parent may be unavailable or postMessage may fail; never break user code.
    }
  }

  function safeLabel(args) {
    if (!args.length || args[0] === undefined) return "default";
    return formatValue(args[0]);
  }

  function now() {
    try {
      if (globalThis.performance && typeof globalThis.performance.now === "function") {
        return globalThis.performance.now();
      }
    } catch (_) { /* use Date below */ }
    return Date.now();
  }

  function report(method, level, args) {
    try {
      post(level, method ? method + (args.length ? ": " : "") + formatArguments(args) : formatArguments(args));
    } catch (_) { /* formatting and forwarding must not affect the program */ }
  }

  function wrap(name, handler) {
    var original;
    try {
      original = globalThis.console[name];
      if (typeof original !== "function") return;
      globalThis.console[name] = function () {
        var args = Array.prototype.slice.call(arguments);
        try {
          return original.apply(globalThis.console, args);
        } finally {
          try { handler(args); } catch (_) { /* do not interfere with console behavior */ }
        }
      };
    } catch (_) {
      // Some environments may expose a read-only console.
    }
  }

  ["log", "info", "warn", "error", "debug", "trace", "dir", "dirxml", "table", "group", "groupCollapsed"].forEach(function (name) {
    var level = name === "info" || name === "warn" || name === "error" || name === "debug" ? name : "log";
    if (name === "trace") level = "debug";
    wrap(name, function (args) { report(["trace", "group", "groupCollapsed"].includes(name) ? name : "", level, args); });
  });

  wrap("assert", function (args) {
    if (!args[0]) {
      var details = args.slice(1);
      post("error", "Assertion failed" + (details.length ? ": " + formatArguments(details) : ""));
    }
  });

  wrap("clear", function () {
    sentMessages = 0;
    forwardingStopped = false;
    post("clear", "Console cleared");
  });

  wrap("count", function (args) {
    var label = safeLabel(args);
    var count = (counts.get(label) || 0) + 1;
    counts.set(label, count);
    post("log", "Count '" + label + "': " + count);
  });

  wrap("countReset", function (args) {
    var label = safeLabel(args);
    counts.set(label, 0);
    post("log", "Count '" + label + "' reset");
  });

  wrap("time", function (args) {
    var label = safeLabel(args);
    timers.set(label, now());
    post("log", "Timer '" + label + "' started");
  });

  wrap("timeLog", function (args) {
    var label = safeLabel(args);
    if (!timers.has(label)) return;
    var elapsed = now() - timers.get(label);
    var extra = args.slice(1);
    post("log", "Timer '" + label + "': " + elapsed + " ms" + (extra.length ? " " + formatArguments(extra) : ""));
  });

  wrap("timeEnd", function (args) {
    var label = safeLabel(args);
    if (!timers.has(label)) return;
    var elapsed = now() - timers.get(label);
    timers.delete(label);
    post("log", "Timer '" + label + "' ended: " + elapsed + " ms");
  });

  function listen(type, handler) {
    try {
      if (typeof globalThis.addEventListener === "function") globalThis.addEventListener(type, handler);
    } catch (_) { /* event reporting is best effort */ }
  }

  listen("error", function (event) {
    try {
      var message = event && event.message;
      var details = [];
      if (message) details.push(String(message));
      if (event && event.filename) details.push(String(event.filename));
      if (event && event.lineno) details.push("line " + String(event.lineno));
      post("error", "Uncaught error" + (details.length ? ": " + details.join(": ") : ""));
    } catch (_) {
      post("error", "Uncaught error");
    }
  });

  listen("unhandledrejection", function (event) {
    try {
      var reason = event && event.reason;
      post("error", "Unhandled promise rejection" + (reason === undefined ? "" : ": " + formatValue(reason)));
    } catch (_) {
      post("error", "Unhandled promise rejection");
    }
  });
})();
