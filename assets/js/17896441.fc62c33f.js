"use strict";(globalThis.webpackChunksimlin_site=globalThis.webpackChunksimlin_site||[]).push([[918],{6138:(e,t,a)=>{a.r(t),a.d(t,{default:()=>x});var n=a(7378),l=a(8944),s=a(9639),i=a(5006),o=a(3298),r=a(3128);const c=function(e){const{metadata:t}=e;return n.createElement("nav",{className:"pagination-nav docusaurus-mt-lg","aria-label":(0,r.I)({id:"theme.docs.paginator.navAriaLabel",message:"Docs pages navigation",description:"The ARIA label for the docs pagination"})},n.createElement("div",{className:"pagination-nav__item"},t.previous&&n.createElement(o.Z,{className:"pagination-nav__link",to:t.previous.permalink},n.createElement("div",{className:"pagination-nav__sublabel"},n.createElement(r.Z,{id:"theme.docs.paginator.previous",description:"The label used to navigate to the previous doc"},"Previous")),n.createElement("div",{className:"pagination-nav__label"},"\xab ",t.previous.title))),n.createElement("div",{className:"pagination-nav__item pagination-nav__item--next"},t.next&&n.createElement(o.Z,{className:"pagination-nav__link",to:t.next.permalink},n.createElement("div",{className:"pagination-nav__sublabel"},n.createElement(r.Z,{id:"theme.docs.paginator.next",description:"The label used to navigate to the next doc"},"Next")),n.createElement("div",{className:"pagination-nav__label"},t.next.title," \xbb"))))};var d=a(9939),m=a(9575);const u={unreleased:function({siteTitle:e,versionMetadata:t}){return n.createElement(r.Z,{id:"theme.docs.versions.unreleasedVersionLabel",description:"The label used to tell the user that he's browsing an unreleased doc version",values:{siteTitle:e,versionLabel:n.createElement("b",null,t.label)}},"This is unreleased documentation for {siteTitle} {versionLabel} version.")},unmaintained:function({siteTitle:e,versionMetadata:t}){return n.createElement(r.Z,{id:"theme.docs.versions.unmaintainedVersionLabel",description:"The label used to tell the user that he's browsing an unmaintained doc version",values:{siteTitle:e,versionLabel:n.createElement("b",null,t.label)}},"This is documentation for {siteTitle} {versionLabel}, which is no longer actively maintained.")}};function v(e){const t=u[e.versionMetadata.banner];return n.createElement(t,e)}function p({versionLabel:e,to:t,onClick:a}){return n.createElement(r.Z,{id:"theme.docs.versions.latestVersionSuggestionLabel",description:"The label used to tell the user to check the latest version",values:{versionLabel:e,latestVersionLink:n.createElement("b",null,n.createElement(o.Z,{to:t,onClick:a},n.createElement(r.Z,{id:"theme.docs.versions.latestVersionLinkLabel",description:"The label used for the latest version suggestion link label"},"latest version")))}},"For up-to-date documentation, see the {latestVersionLink} ({versionLabel}).")}function h({versionMetadata:e}){const{siteConfig:{title:t}}=(0,d.Z)(),{pluginId:a}=(0,s.gA)({failfast:!0}),{savePreferredVersionName:l}=(0,m.J)(a),{latestDocSuggestion:i,latestVersionSuggestion:o}=(0,s.Jo)(a),r=null!=i?i:(c=o).docs.find((e=>e.id===c.mainDocId));var c;return n.createElement("div",{className:"alert alert--warning margin-bottom--md",role:"alert"},n.createElement("div",null,n.createElement(v,{siteTitle:t,versionMetadata:e})),n.createElement("div",{className:"margin-top--md"},n.createElement(p,{versionLabel:o.label,to:r.path,onClick:()=>l(o.name)})))}const b=function({versionMetadata:e}){return"none"===e.banner?n.createElement(n.Fragment,null):n.createElement(h,{versionMetadata:e})};var E=a(7165);function g({lastUpdatedAt:e,formattedLastUpdatedAt:t}){return n.createElement(r.Z,{id:"theme.lastUpdated.atDate",description:"The words used to describe on which date a page has been last updated",values:{date:n.createElement("b",null,n.createElement("time",{dateTime:new Date(1e3*e).toISOString()},t))}}," on {date}")}function f({lastUpdatedBy:e}){return n.createElement(r.Z,{id:"theme.lastUpdated.byUser",description:"The words used to describe by who the page has been last updated",values:{user:n.createElement("b",null,e)}}," by {user}")}function _({lastUpdatedAt:e,formattedLastUpdatedAt:t,lastUpdatedBy:a}){return n.createElement(n.Fragment,null,n.createElement(r.Z,{id:"theme.lastUpdated.lastUpdatedAtBy",description:"The sentence used to display when a page has been last updated, and by who",values:{atDate:e&&t?n.createElement(g,{lastUpdatedAt:e,formattedLastUpdatedAt:t}):"",byUser:a?n.createElement(f,{lastUpdatedBy:a}):""}},"Last updated{atDate}{byUser}"),!1)}var N=a(2079);const L="tocCollapsible_Snzk",C="tocCollapsibleButton_27DV",T="tocCollapsibleContent_6RV4",U="tocCollapsibleExpanded__mI0";function Z({toc:e,className:t}){const{collapsed:a,toggleCollapsed:s}=(0,m.uR)({initialState:!0});return n.createElement("div",{className:(0,l.Z)(L,{[U]:!a},t)},n.createElement("button",{type:"button",className:(0,l.Z)("clean-btn",C),onClick:s},n.createElement(r.Z,{id:"theme.TOCCollapsible.toggleButtonLabel",description:"The label used by the button on the collapsible TOC component"},"On this page")),n.createElement(m.zF,{lazy:!0,className:T,collapsed:a},n.createElement(N.r,{toc:e})))}var k=a(5561),y=a(8638);const w="docItemContainer_3nUq",A="lastUpdated_24hC",B="docItemCol_1o9i",M="tocMobile_1BQl";const x=function(e){const{content:t,versionMetadata:a}=e,{metadata:o,frontMatter:r}=t,{image:d,keywords:m,hide_title:u,hide_table_of_contents:v}=r,{description:p,title:h,editUrl:g,lastUpdatedAt:f,formattedLastUpdatedAt:L,lastUpdatedBy:C}=o,{pluginId:T}=(0,s.gA)({failfast:!0}),U=(0,s.gB)(T).length>1,x=!u&&void 0===t.contentTitle,I=(0,i.Z)(),V=!v&&t.toc&&t.toc.length>0,S=V&&("desktop"===I||"ssr"===I);return n.createElement(n.Fragment,null,n.createElement(E.Z,{title:h,description:p,keywords:m,image:d}),n.createElement("div",{className:"row"},n.createElement("div",{className:(0,l.Z)("col",{[B]:!v})},n.createElement(b,{versionMetadata:a}),n.createElement("div",{className:w},n.createElement("article",null,U&&n.createElement("span",{className:"badge badge--secondary"},"Version: ",a.label),V&&n.createElement(Z,{toc:t.toc,className:M}),n.createElement("div",{className:"markdown"},x&&n.createElement(y.N,null,h),n.createElement(t,null)),(g||f||C)&&n.createElement("footer",{className:"row docusaurus-mt-lg"},n.createElement("div",{className:"col"},g&&n.createElement(k.Z,{editUrl:g})),n.createElement("div",{className:(0,l.Z)("col",A)},(f||C)&&n.createElement(_,{lastUpdatedAt:f,formattedLastUpdatedAt:L,lastUpdatedBy:C})))),n.createElement(c,{metadata:o}))),S&&n.createElement("div",{className:"col col--3"},n.createElement(N.Z,{toc:t.toc}))))}},5561:(e,t,a)=>{a.d(t,{Z:()=>c});var n=a(7378),l=a(3128),s=a(5773),i=a(8944);const o="iconEdit_1CBY",r=({className:e,...t})=>n.createElement("svg",(0,s.Z)({fill:"currentColor",height:"20",width:"20",viewBox:"0 0 40 40",className:(0,i.Z)(o,e),"aria-hidden":"true"},t),n.createElement("g",null,n.createElement("path",{d:"m34.5 11.7l-3 3.1-6.3-6.3 3.1-3q0.5-0.5 1.2-0.5t1.1 0.5l3.9 3.9q0.5 0.4 0.5 1.1t-0.5 1.2z m-29.5 17.1l18.4-18.5 6.3 6.3-18.4 18.4h-6.3v-6.2z"})));function c({editUrl:e}){return n.createElement("a",{href:e,target:"_blank",rel:"noreferrer noopener"},n.createElement(r,null),n.createElement(l.Z,{id:"theme.common.editThisPage",description:"The link label to edit the current page"},"Edit this page"))}},2079:(e,t,a)=>{a.d(t,{r:()=>r,Z:()=>c});var n=a(7378),l=a(8944);const s=function(e,t,a){const[l,s]=(0,n.useState)(void 0);(0,n.useEffect)((()=>{function n(){const n=function(){const e=Array.from(document.getElementsByClassName("anchor")),t=e.find((e=>{const{top:t}=e.getBoundingClientRect();return t>=a}));if(t){if(t.getBoundingClientRect().top>=a){const a=e[e.indexOf(t)-1];return null!=a?a:t}return t}return e[e.length-1]}();if(n){let a=0,i=!1;const o=document.getElementsByClassName(e);for(;a<o.length&&!i;){const e=o[a],{href:r}=e,c=decodeURIComponent(r.substring(r.indexOf("#")+1));n.id===c&&(l&&l.classList.remove(t),e.classList.add(t),s(e),i=!0),a+=1}}}return document.addEventListener("scroll",n),document.addEventListener("resize",n),n(),()=>{document.removeEventListener("scroll",n),document.removeEventListener("resize",n)}}))},i="tableOfContents_3J2a",o="table-of-contents__link";function r({toc:e,isChild:t}){return e.length?n.createElement("ul",{className:t?"":"table-of-contents table-of-contents__left-border"},e.map((e=>n.createElement("li",{key:e.id},n.createElement("a",{href:`#${e.id}`,className:o,dangerouslySetInnerHTML:{__html:e.value}}),n.createElement(r,{isChild:!0,toc:e.children}))))):null}const c=function({toc:e}){return s(o,"table-of-contents__link--active",100),n.createElement("div",{className:(0,l.Z)(i,"thin-scrollbar")},n.createElement(r,{toc:e}))}}}]);