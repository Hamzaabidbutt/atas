/* ATAS clone — shared UI behaviour */
(function () {
  'use strict';

  var reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* --- Sticky header shadow ---------------------------------------------- */
  var header = document.querySelector('.site-header');
  if (header) {
    var onScroll = function () {
      header.classList.toggle('is-stuck', window.scrollY > 8);
    };
    onScroll();
    window.addEventListener('scroll', onScroll, { passive: true });
  }

  /* --- Mobile nav --------------------------------------------------------- */
  var toggle = document.querySelector('.nav-toggle');
  var links = document.getElementById('nav-links');
  if (toggle && links) {
    toggle.addEventListener('click', function () {
      var open = links.classList.toggle('open');
      toggle.setAttribute('aria-expanded', String(open));
    });
    links.addEventListener('click', function (e) {
      if (e.target.closest('a')) {
        links.classList.remove('open');
        toggle.setAttribute('aria-expanded', 'false');
      }
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && links.classList.contains('open')) {
        links.classList.remove('open');
        toggle.setAttribute('aria-expanded', 'false');
        toggle.focus();
      }
    });
  }

  /* --- Year stamp --------------------------------------------------------- */
  Array.prototype.forEach.call(document.querySelectorAll('[data-year]'), function (el) {
    el.textContent = String(new Date().getFullYear());
  });

  /* --- Scroll reveal ------------------------------------------------------ */
  var revealables = document.querySelectorAll('.reveal');
  if (revealables.length) {
    if (reduceMotion || !('IntersectionObserver' in window)) {
      Array.prototype.forEach.call(revealables, function (el) { el.classList.add('in'); });
    } else {
      var io = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          if (entry.isIntersecting) {
            entry.target.classList.add('in');
            io.unobserve(entry.target);
          }
        });
      }, { rootMargin: '0px 0px -8% 0px', threshold: 0.06 });
      Array.prototype.forEach.call(revealables, function (el, i) {
        el.style.transitionDelay = Math.min(i % 4, 3) * 70 + 'ms';
        io.observe(el);
      });
    }
  }

  /* --- Tabs --------------------------------------------------------------- */
  Array.prototype.forEach.call(document.querySelectorAll('[data-tabs]'), function (group) {
    var tabs = group.querySelectorAll('[role="tab"]');

    function select(tab) {
      Array.prototype.forEach.call(tabs, function (t) {
        var on = t === tab;
        t.setAttribute('aria-selected', String(on));
        t.tabIndex = on ? 0 : -1;
        var panel = document.getElementById(t.getAttribute('aria-controls'));
        if (panel) panel.hidden = !on;
      });
    }

    Array.prototype.forEach.call(tabs, function (tab, i) {
      tab.addEventListener('click', function () { select(tab); });
      tab.addEventListener('keydown', function (e) {
        var dir = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
        if (!dir) return;
        e.preventDefault();
        var next = tabs[(i + dir + tabs.length) % tabs.length];
        next.focus();
        select(next);
      });
    });
  });

  /* --- Pricing billing toggle -------------------------------------------- */
  var billing = document.querySelector('[data-billing]');
  if (billing) {
    var buttons = billing.querySelectorAll('button[data-period]');
    var apply = function (period) {
      Array.prototype.forEach.call(buttons, function (b) {
        b.setAttribute('aria-pressed', String(b.dataset.period === period));
      });
      Array.prototype.forEach.call(document.querySelectorAll('[data-monthly]'), function (el) {
        var value = period === 'yearly' ? el.dataset.yearly : el.dataset.monthly;
        el.textContent = value;
      });
      Array.prototype.forEach.call(document.querySelectorAll('[data-note-monthly]'), function (el) {
        el.textContent = period === 'yearly'
          ? el.dataset.noteYearly
          : el.dataset.noteMonthly;
      });
    };
    Array.prototype.forEach.call(buttons, function (b) {
      b.addEventListener('click', function () { apply(b.dataset.period); });
    });
    apply('monthly');
  }

  /* --- Animated Time & Sales tape ---------------------------------------- */
  var tape = document.getElementById('tape');
  if (tape && !reduceMotion) {
    var price = 5312.25;
    var MAX_ROWS = 16;

    var pad = function (n) { return n < 10 ? '0' + n : String(n); };

    function row() {
      var drift = (Math.random() - 0.48) * 0.75;
      price = Math.max(5290, Math.min(5335, price + drift));
      var buy = Math.random() > 0.48;
      var size = Math.max(1, Math.round(Math.pow(Math.random(), 3.1) * 340));
      var now = new Date();

      var li = document.createElement('li');
      li.className = (buy ? 'buy' : 'sell') + (size > 120 ? ' big' : '');
      li.innerHTML =
        '<span class="px">' + price.toFixed(2) + '</span>' +
        '<span class="sz">' + size + '</span>' +
        '<span class="tm">' + pad(now.getMinutes()) + ':' + pad(now.getSeconds()) + '</span>';

      tape.insertBefore(li, tape.firstChild);
      while (tape.children.length > MAX_ROWS) tape.removeChild(tape.lastChild);
    }

    for (var i = 0; i < MAX_ROWS; i++) row();

    var timer = null;
    var tick = function () {
      row();
      timer = setTimeout(tick, 320 + Math.random() * 620);
    };
    tick();

    document.addEventListener('visibilitychange', function () {
      if (document.hidden) {
        clearTimeout(timer);
      } else if (timer !== null) {
        clearTimeout(timer);
        tick();
      }
    });
  }
})();
