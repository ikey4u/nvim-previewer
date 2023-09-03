<script id="MathJax-script" async src="https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-svg.js"></script>
<script>
  MathJax = {
    tex: {
      inlineMath: [
        ['$', '$'],
      ]
    },
    svg: {
      fontCache: 'none',
      exFactor: 1,
    },
    options: {
      renderActions: {
        assistiveMml: []
      }
    }
  };

  function exportHtml() {
    let str = document.getElementById('content').innerHTML
    function listener(e) {
      e.clipboardData.setData("text/html", str);
      e.clipboardData.setData("text/plain", str);
      e.preventDefault();
    }
    document.addEventListener("copy", listener);
    document.execCommand("copy");
    document.removeEventListener("copy", listener);
  }

  document.addEventListener('DOMContentLoaded', function(event) {
    // placeholder
  });

  window.addEventListener('load', function() {
    // Let's make it compatible with the troublesome WeChat Official Account
    for (let mjx of document.querySelectorAll("mjx-container[display='true']")) {
      mjx.getElementsByTagName("svg")[0].style.width = "100%";
    }

    // Add space around inline math equation
    for (let mjx of document.querySelectorAll("mjx-container:not([display])")) {
      mjx.style["margin-left"] = "0.5em";
      mjx.style["margin-right"] = "0.5em";
    }
  });
</script>

